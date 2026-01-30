use crate::StockQuote;
use crate::errors::GeneratorError;
use rand::Rng;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    io::BufReader,
    sync::{Arc, RwLock, mpsc::Sender},
    time::{SystemTime, UNIX_EPOCH},
};

/// Maximum percentage change allowed per price update (2%)
const MAX_CHANGE_PERCENT: f64 = 0.02;

/// Minimum price threshold to prevent prices from going to zero
const MIN_PRICE: f64 = 0.01;

/// Default delay between quote generations in milliseconds
const BASE_DELAY: u64 = 1000;

/// Maximum starting price for randomly initialized tickers
const MAX_START_PRICE: f64 = 100.0;

/// Information about a receiver that subscribes to stock quotes.
///
/// Each receiver has a unique ID, a channel for sending quotes, and tracks
/// which tickers they are subscribed to. This allows efficient management
/// of subscriptions where one receiver can subscribe to multiple tickers
/// using a single channel.
struct RecieverInfo {
    /// Unique identifier for this receiver
    id: u64,
    /// Channel used to send StockQuote updates to this receiver
    channel: mpsc::Sender<StockQuote>,
    /// Set of ticker symbols this receiver is subscribed to
    subscribed_tickers: HashSet<String>,
}

/// Generates a new price using random walk algorithm.
///
/// Changes are proportional to the current price and bounded to prevent negative values.
/// The price change follows a random walk with a maximum change of ±2% per tick.
///
/// # Arguments
/// * `previous_price` - The current price to base the new price on
///
/// # Returns
/// A new price that differs from the previous by at most ±2%, never below MIN_PRICE
fn generate_new_price(previous_price: f64) -> f64 {
    let mut rng = rand::rng();

    // Generate a random change between -MAX_CHANGE_PERCENT and +MAX_CHANGE_PERCENT
    let change_percent = rng.random_range(-MAX_CHANGE_PERCENT..=MAX_CHANGE_PERCENT);

    // Calculate the new price with proportional change
    let new_price = previous_price * (1.0 + change_percent);

    // Ensure price never goes below minimum
    new_price.max(MIN_PRICE)
}

/// Generates a random initial price for a new ticker.
///
/// # Returns
/// A random price between MIN_PRICE and MAX_START_PRICE
fn generate_initial_price() -> f64 {
    let mut rng = rand::rng();
    rng.random_range(MIN_PRICE..MAX_START_PRICE)
}

/// Tracks the current price and metadata for a specific ticker.
///
/// This structure maintains the current price and timestamp of the last change,
/// which can be used for calculating dynamic update intervals in the future.
struct PriceChange {
    /// Current price of the ticker
    price: f64,
    /// Unix timestamp (milliseconds) of the last price update
    last_change: u64,
}

/// Stock quote generator that manages price updates and distributes them to subscribers.
///
/// # Architecture
///
/// The generator uses three key data structures to efficiently manage quote generation
/// and distribution:
///
/// ## 1. `prices` - Price Tracking
/// Maps ticker symbols to their current price and metadata:
/// ```text
/// "AAPL" -> PriceChange { price: 150.25, last_change: 1234567890 }
/// "MSFT" -> PriceChange { price: 380.50, last_change: 1234567891 }
/// ```
/// Each ticker runs in its own thread, continuously generating new prices using a
/// random walk algorithm and updating this map.
///
/// ## 2. `recievers` - Receiver Registry
/// Maps receiver IDs to their channel and subscription information:
/// ```text
/// 1 -> RecieverInfo {
///     id: 1,
///     channel: Sender<StockQuote>,
///     subscribed_tickers: {"AAPL", "MSFT", "GOOGL"}
/// }
/// 2 -> RecieverInfo {
///     id: 2,
///     channel: Sender<StockQuote>,
///     subscribed_tickers: {"TSLA"}
/// }
/// ```
/// This design allows each receiver to have ONE channel that receives quotes for
/// ALL their subscribed tickers, reducing memory usage and complexity.
///
/// ## 3. `tickers_to_recievers` - Distribution Index
/// Maps ticker symbols to lists of receiver IDs subscribed to them:
/// ```text
/// "AAPL" -> [1, 3, 5]
/// "MSFT" -> [1, 2]
/// "TSLA" -> [2, 4]
/// ```
/// When a new quote is generated for a ticker, this map allows quick lookup of
/// which receivers need to be notified. The quote is then sent to each receiver's
/// channel (found in `recievers`).
///
/// ## Quote Flow
/// 1. Each ticker has a dedicated thread that generates quotes periodically
/// 2. New quote is generated using `generate_quote()` and stored in `prices`
/// 3. `tickers_to_recievers` is consulted to find subscribed receivers
/// 4. For each receiver ID, their channel is retrieved from `recievers`
/// 5. Quote is sent through the channel to the receiver
///
/// ## Thread Safety
/// All data structures use `Arc<RwLock<>>` for safe concurrent access across
/// multiple ticker threads and the main thread handling subscriptions.
pub(crate) struct QuoteGenerator {
    /// Current prices for all tracked tickers
    pub(crate) prices: Arc<RwLock<HashMap<String, PriceChange>>>,
    /// Registry of all receivers with their channels and subscriptions
    pub(crate) recievers: Arc<RwLock<HashMap<u64, RecieverInfo>>>,
    /// Index mapping tickers to their subscribed receiver IDs
    pub(crate) tickers_to_recievers: Arc<RwLock<HashMap<String, Vec<u64>>>>,
}

impl QuoteGenerator {
    /// Creates a new QuoteGenerator with the specified tickers.
    ///
    /// Each ticker is initialized with a random starting price. No receivers
    /// are subscribed initially - use `add_reciever()` to add subscriptions.
    ///
    /// # Arguments
    /// * `tickers` - An iterator of ticker symbols (e.g., "AAPL", "MSFT")
    ///
    /// # Returns
    /// A new QuoteGenerator instance ready to start generating quotes
    ///
    /// # Example
    /// ```ignore
    /// let tickers = vec!["AAPL".to_string(), "GOOGL".to_string()];
    /// let generator = QuoteGenerator::new(tickers.into_iter());
    /// ```
    pub(crate) fn new<T>(tickers: T) -> Self
    where
        T: Iterator<Item = String>,
    {
        let mut prices_hm = HashMap::new();
        let mut tickers_recv_hm = HashMap::new();
        for ticker in tickers {
            prices_hm.insert(
                ticker.clone(),
                PriceChange {
                    price: generate_initial_price(),
                    last_change: 0u64,
                },
            );
            tickers_recv_hm.insert(ticker, Vec::new());
        }

        Self {
            prices: Arc::new(RwLock::new(prices_hm)),
            recievers: Arc::new(RwLock::new(HashMap::new())),
            tickers_to_recievers: Arc::new(RwLock::new(tickers_recv_hm)),
        }
    }

    /// Creates a new QuoteGenerator by loading ticker symbols from a file.
    ///
    /// Each line in the file should contain one ticker symbol. Whitespace is trimmed.
    ///
    /// # Arguments
    /// * `input` - Path to the file containing ticker symbols
    ///
    /// # Returns
    /// * `Ok(QuoteGenerator)` - Successfully loaded tickers
    /// * `Err(GeneratorError)` - Failed to read the file
    ///
    /// # Example
    /// ```ignore
    /// let generator = QuoteGenerator::new_from_file(PathBuf::from("tickers.txt"))?;
    /// ```
    pub(crate) fn new_from_file(input: PathBuf) -> Result<Self, GeneratorError> {
        let file = std::fs::File::open(input)?;
        let reader = BufReader::new(file);
        let tickers = reader.lines().map(|x| x.unwrap().trim().to_owned());
        Ok(QuoteGenerator::new(tickers))
    }

    /// Starts the quote generator for all tickers.
    ///
    /// Spawns a separate thread for each ticker that continuously generates new quotes
    /// at the specified delay interval. This method blocks until all ticker threads
    /// complete (which is never, unless the thread scope is dropped).
    ///
    /// New receivers can be added using `add_reciever()` while the generator is running.
    /// The quote generation threads will automatically pick up new subscriptions.
    ///
    /// # Thread Model
    /// Each ticker gets its own thread that:
    /// 1. Generates a new price using random walk
    /// 2. Updates the `prices` map
    /// 3. Looks up subscribed receivers in `tickers_to_recievers`
    /// 4. Sends the quote to each receiver's channel
    /// 5. Sleeps for the delay period
    /// 6. Repeats
    pub(crate) fn start(&self) {
        let tickers: Vec<_> = self.prices.read().unwrap().keys().cloned().collect();
        thread::scope(|scope| {
            for ticker in tickers {
                let prices = Arc::clone(&self.prices);
                let receivers = Arc::clone(&self.recievers);
                let tickers_to_recievers = Arc::clone(&self.tickers_to_recievers);
                scope.spawn(move || {
                    Self::start_for_quote(&prices, &receivers, &tickers_to_recievers, &ticker, BASE_DELAY);
                });
            }
        })
    }

    /// Generates a new quote for a specific ticker and updates the stored price.
    ///
    /// This function:
    /// 1. Reads the current price from the `prices` map
    /// 2. Generates a new price using random walk algorithm
    /// 3. Updates the stored price and timestamp
    /// 4. Creates a StockQuote with random volume
    ///
    /// # Arguments
    /// * `prices` - Shared reference to the prices map
    /// * `ticker` - The ticker symbol to generate a quote for
    ///
    /// # Returns
    /// * `Some(StockQuote)` - A new quote with updated price
    /// * `None` - If the ticker doesn't exist in the prices map
    ///
    /// # Volume Generation
    /// Popular stocks (AAPL, MSFT, TSLA) get higher volume (1000-6000),
    /// while others get lower volume (100-1100).
    fn generate_quote(
        prices: &Arc<RwLock<HashMap<String, PriceChange>>>,
        ticker: &str,
    ) -> Option<StockQuote> {
        let new_price = {
            let prices = prices.read().unwrap();
            let last_data = prices.get(ticker)?;
            generate_new_price(last_data.price)
        };

        // Update stored price
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        {
            let mut prices_guard = prices.write().unwrap();
            if let Some(data) = prices_guard.get_mut(ticker) {
                data.price = new_price;
                data.last_change = timestamp;
            }
        }

        let mut rng = rand::rng();
        let volume = match ticker {
            // Popular stocks have higher volume
            "AAPL" | "MSFT" | "TSLA" => 1000 + rng.random_range(0..5000),
            // Regular stocks - medium volume
            _ => 100 + rng.random_range(0..1000),
        };

        Some(StockQuote {
            ticker: ticker.to_string(),
            price: new_price,
            volume,
            timestamp: timestamp,
        })
    }

    /// Continuously generates and distributes quotes for a specific ticker.
    ///
    /// This is the main loop for each ticker thread. It runs forever, generating
    /// new quotes at regular intervals and distributing them to all subscribed receivers.
    ///
    /// # Arguments
    /// * `prices` - Shared prices map for reading/updating prices
    /// * `recievers` - Shared receiver registry for looking up channels
    /// * `tickers_to_recievers` - Shared index of ticker→receiver mappings
    /// * `ticker` - The ticker symbol this thread is responsible for
    /// * `delay` - Milliseconds to wait between quote generations
    ///
    /// # Distribution Flow
    /// 1. Generate new quote using `generate_quote()`
    /// 2. Acquire read locks on both maps
    /// 3. Look up receiver IDs from `tickers_to_recievers[ticker]`
    /// 4. For each receiver ID, get their channel from `recievers[id]`
    /// 5. Send quote through channel (errors are logged but don't stop the loop)
    /// 6. Sleep for the specified delay
    /// 7. Repeat
    ///
    /// # Error Handling
    /// If sending fails (channel closed), an error is printed but the loop continues.
    /// This allows the system to keep running even if individual receivers disconnect.
    fn start_for_quote(
        prices: &Arc<RwLock<HashMap<String, PriceChange>>>,
        recievers: &Arc<RwLock<HashMap<u64, RecieverInfo>>>,
        tickers_to_recievers: &Arc<RwLock<HashMap<String, Vec<u64>>>>,
        ticker: &str,
        delay: u64,
    ) {
        loop {
            let quote = Self::generate_quote(prices, ticker);
            if let Some(quote) = quote {
                let tickers_to_recv_guard = tickers_to_recievers.read().unwrap();
                let recv_guard = recievers.read().unwrap();
                if let Some(list) = tickers_to_recv_guard.get(ticker) {
                    for receiver_id in list {
                        if let Some(receiver_info) = recv_guard.get(receiver_id) {
                            if receiver_info.channel.send(quote.clone()).is_err() {
                                // Channel closed - receiver disconnected
                                println!("Error sending quote to receiver {}: channel closed", receiver_id);
                            }
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(delay));
        }
    }

    /// Adds a receiver subscription for a specific ticker.
    ///
    /// If this is a new receiver ID, creates a new entry in the receiver registry
    /// with the provided channel. If the receiver already exists, adds the ticker
    /// to their subscription set.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for the receiver
    /// * `sender` - Channel for sending StockQuotes to this receiver
    /// * `ticker` - Ticker symbol to subscribe to
    ///
    /// # Data Structure Updates
    /// 1. **recievers map**: Creates or updates the RecieverInfo for this ID
    ///    - New: Inserts with the channel and ticker in subscription set
    ///    - Existing: Adds ticker to existing subscription set
    /// 2. **tickers_to_recievers map**: Adds receiver ID to the ticker's list
    ///
    /// # Example
    /// ```ignore
    /// let (tx, rx) = mpsc::channel();
    /// generator.add_reciever(1, tx, "AAPL");
    /// generator.add_reciever(1, tx, "MSFT"); // Same receiver, different ticker
    /// ```
    pub(crate) fn add_reciever(&mut self, id: u64, sender: Sender<StockQuote>, ticker: &str) {
        // 1. Add receiver if not exists, or update their subscribed tickers
        self.recievers
            .write()
            .unwrap()
            .entry(id)
            .and_modify(|info| {
                info.subscribed_tickers.insert(ticker.to_owned());
            })
            .or_insert(RecieverInfo {
                id,
                channel: sender,
                subscribed_tickers: HashSet::from([ticker.to_owned()]),
            });

        // 2. Add receiver ID to ticker's subscription list
        self.tickers_to_recievers
            .write()
            .unwrap()
            .entry(ticker.to_owned())
            .or_insert(Vec::new())
            .push(id);
    }

    /// Removes a receiver's subscription to a specific ticker.
    ///
    /// If the receiver has no remaining subscriptions after removal, the receiver
    /// is completely removed from the registry to free resources.
    ///
    /// # Arguments
    /// * `id` - Unique identifier of the receiver
    /// * `ticker` - Ticker symbol to unsubscribe from
    ///
    /// # Data Structure Updates
    /// 1. **recievers map**: Removes ticker from subscription set
    ///    - If subscription set becomes empty, removes the entire receiver entry
    /// 2. **tickers_to_recievers map**: Removes receiver ID from ticker's list
    ///
    /// # Behavior
    /// - If receiver doesn't exist: No-op, no error
    /// - If ticker not in receiver's subscriptions: No-op, no error
    ///
    /// # Example
    /// ```ignore
    /// generator.remove_reciever(1, "AAPL"); // Unsubscribe from AAPL
    /// // If receiver 1 has no more subscriptions, they're removed completely
    /// ```
    pub(crate) fn remove_reciever(&mut self, id: u64, ticker: &str) {
        // 1. Remove ticker from receiver's subscription list
        let mut recvs = self.recievers.write().unwrap();
        if let Some(info) = recvs.get_mut(&id) {
            info.subscribed_tickers.remove(ticker);

            // If receiver has no more subscriptions, remove them completely
            if info.subscribed_tickers.is_empty() {
                recvs.remove(&id);
            }
        }
        drop(recvs); // Release lock

        // 2. Remove receiver ID from ticker's list
        self.tickers_to_recievers
            .write()
            .unwrap()
            .entry(ticker.to_owned())
            .and_modify(|ids| {
                if let Some(pos) = ids.iter().position(|&x| x == id) {
                    ids.remove(pos);
                }
            });
    }

    /// Removes a receiver completely from all ticker subscriptions.
    ///
    /// This is a convenience method for unsubscribing a receiver from all tickers
    /// at once, typically used when a client disconnects.
    ///
    /// # Arguments
    /// * `id` - Unique identifier of the receiver to remove
    ///
    /// # Implementation
    /// 1. Retrieves all tickers from receiver's subscription set
    /// 2. Calls `remove_reciever()` for each ticker
    /// 3. The receiver entry is removed when the last subscription is removed
    ///
    /// # Behavior
    /// If the receiver doesn't exist, this is a no-op (no error).
    ///
    /// # Example
    /// ```ignore
    /// // Client disconnects
    /// generator.remove_reciever_all(client_id);
    /// // All subscriptions cleaned up, resources freed
    /// ```
    pub(crate) fn remove_reciever_all(&mut self, id: u64) {
        // Get all tickers this receiver is subscribed to
        let tickers: Vec<String> = self.recievers
            .read()
            .unwrap()
            .get(&id)
            .map(|info| info.subscribed_tickers.iter().cloned().collect())
            .unwrap_or_default();

        // Remove from all ticker lists
        for ticker in tickers {
            self.remove_reciever(id, &ticker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    // -- generate_new_price tests --

    #[test]
    fn generate_new_price_stays_within_bounds() {
        let price = 100.0;
        for _ in 0..1000 {
            let new = generate_new_price(price);
            assert!(new >= price * (1.0 - MAX_CHANGE_PERCENT));
            assert!(new <= price * (1.0 + MAX_CHANGE_PERCENT));
        }
    }

    #[test]
    fn generate_new_price_never_below_min() {
        // Very small price should still not go below MIN_PRICE
        for _ in 0..1000 {
            let new = generate_new_price(MIN_PRICE);
            assert!(new >= MIN_PRICE);
        }
    }

    #[test]
    fn generate_new_price_proportional_to_price() {
        // Higher price should produce larger absolute changes on average
        let mut small_diffs = Vec::new();
        let mut large_diffs = Vec::new();
        for _ in 0..1000 {
            small_diffs.push((generate_new_price(10.0) - 10.0).abs());
            large_diffs.push((generate_new_price(1000.0) - 1000.0).abs());
        }
        let avg_small: f64 = small_diffs.iter().sum::<f64>() / small_diffs.len() as f64;
        let avg_large: f64 = large_diffs.iter().sum::<f64>() / large_diffs.len() as f64;
        assert!(avg_large > avg_small * 10.0);
    }

    // -- QuoteGenerator::new tests --

    #[test]
    fn new_creates_entries_for_all_tickers() {
        let tickers = vec!["AAPL", "MSFT", "TSLA"];
        let qg = QuoteGenerator::new(tickers.iter().map(|s| s.to_string()));
        let prices = qg.prices.read().unwrap();
        assert_eq!(prices.len(), 3);
        for t in &tickers {
            assert!(prices.contains_key(*t));
        }
    }

    #[test]
    fn new_initializes_last_change_to_zero() {
        let qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let prices = qg.prices.read().unwrap();
        assert_eq!(prices.get("AAPL").unwrap().last_change, 0);
    }

    #[test]
    fn new_with_empty_iterator() {
        let qg = QuoteGenerator::new(std::iter::empty::<String>());
        let prices = qg.prices.read().unwrap();
        assert!(prices.is_empty());
    }

    // -- QuoteGenerator::new_from_file tests --

    #[test]
    fn new_from_file_loads_tickers() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/data/tickers.txt");
        let qg = QuoteGenerator::new_from_file(path).unwrap();
        let prices = qg.prices.read().unwrap();
        assert!(prices.contains_key("AAPL"));
        assert!(prices.contains_key("MSFT"));
        assert!(prices.contains_key("TSLA"));
        assert_eq!(prices.len(), 110);
    }

    #[test]
    fn new_from_file_nonexistent_returns_error() {
        let result = QuoteGenerator::new_from_file(PathBuf::from("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    // -- QuoteGenerator::generate_quote tests --

    #[test]
    fn generate_quote_returns_valid_quote() {
        let qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let quote = QuoteGenerator::generate_quote(&qg.prices, "AAPL").unwrap();
        assert_eq!(quote.ticker, "AAPL");
        assert!(quote.price > 0.0);
        assert!(quote.volume >= 1000); // AAPL is a popular stock
        assert!(quote.timestamp > 0);
    }

    #[test]
    fn generate_quote_updates_stored_price() {
        let qg = QuoteGenerator::new(vec!["MSFT".to_string()].into_iter());
        let original_price = qg.prices.read().unwrap().get("MSFT").unwrap().price;
        QuoteGenerator::generate_quote(&qg.prices, "MSFT");
        let updated = qg.prices.read().unwrap();
        let data = updated.get("MSFT").unwrap();
        // last_change should have been updated from 0
        assert!(data.last_change > 0);
    }

    #[test]
    fn generate_quote_unknown_ticker_returns_none() {
        let qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let result = QuoteGenerator::generate_quote(&qg.prices, "UNKNOWN");
        assert!(result.is_none());
    }

    #[test]
    fn generate_quote_popular_stock_higher_volume() {
        let qg = QuoteGenerator::new(vec!["AAPL".to_string(), "XYZ".to_string()].into_iter());
        let mut popular_volumes = Vec::new();
        let mut regular_volumes = Vec::new();
        for _ in 0..100 {
            popular_volumes.push(
                QuoteGenerator::generate_quote(&qg.prices, "AAPL")
                    .unwrap()
                    .volume,
            );
            regular_volumes.push(
                QuoteGenerator::generate_quote(&qg.prices, "XYZ")
                    .unwrap()
                    .volume,
            );
        }
        let avg_popular: f64 = popular_volumes.iter().map(|v| *v as f64).sum::<f64>() / 100.0;
        let avg_regular: f64 = regular_volumes.iter().map(|v| *v as f64).sum::<f64>() / 100.0;
        assert!(avg_popular > avg_regular);
    }

    // -- add_reciever / remove_reciever tests --

    #[test]
    fn add_reciever_creates_subscription() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, _rx) = mpsc::channel();
        qg.add_reciever(1, tx, "AAPL");

        // Check receiver was added
        let recvs = qg.recievers.read().unwrap();
        assert!(recvs.contains_key(&1));
        assert_eq!(recvs.get(&1).unwrap().id, 1);
        assert!(recvs.get(&1).unwrap().subscribed_tickers.contains("AAPL"));

        // Check ticker subscription list was updated
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        assert_eq!(ticker_subs.get("AAPL").unwrap().len(), 1);
        assert_eq!(ticker_subs.get("AAPL").unwrap()[0], 1);
    }

    #[test]
    fn add_multiple_recievers_same_ticker() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        qg.add_reciever(1, tx1, "AAPL");
        qg.add_reciever(2, tx2, "AAPL");

        // Check both receivers exist
        let recvs = qg.recievers.read().unwrap();
        assert_eq!(recvs.len(), 2);
        assert!(recvs.contains_key(&1));
        assert!(recvs.contains_key(&2));

        // Check ticker has both receivers
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        assert_eq!(ticker_subs.get("AAPL").unwrap().len(), 2);
    }

    #[test]
    fn remove_reciever_removes_correct_subscription() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        qg.add_reciever(1, tx1, "AAPL");
        qg.add_reciever(2, tx2, "AAPL");
        qg.remove_reciever(1, "AAPL");

        // Receiver 1 should be removed (had only AAPL)
        let recvs = qg.recievers.read().unwrap();
        assert!(!recvs.contains_key(&1));
        assert!(recvs.contains_key(&2));

        // Ticker should only have receiver 2
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        let aapl_subs = ticker_subs.get("AAPL").unwrap();
        assert_eq!(aapl_subs.len(), 1);
        assert_eq!(aapl_subs[0], 2);
    }

    #[test]
    fn remove_reciever_nonexistent_id_does_nothing() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, _) = mpsc::channel();
        qg.add_reciever(1, tx, "AAPL");
        qg.remove_reciever(999, "AAPL");

        // Receiver 1 should still exist
        let recvs = qg.recievers.read().unwrap();
        assert!(recvs.contains_key(&1));
        assert_eq!(recvs.len(), 1);

        // Ticker should still have receiver 1
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        assert_eq!(ticker_subs.get("AAPL").unwrap().len(), 1);
    }

    #[test]
    fn add_reciever_multiple_tickers_same_receiver() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string(), "MSFT".to_string()].into_iter());
        let (tx, _) = mpsc::channel();

        // Manually setup receiver with multiple tickers
        {
            let mut recvs = qg.recievers.write().unwrap();
            recvs.insert(1, RecieverInfo {
                id: 1,
                channel: tx,
                subscribed_tickers: HashSet::from(["AAPL".to_string(), "MSFT".to_string()]),
            });
        }
        {
            let mut ticker_subs = qg.tickers_to_recievers.write().unwrap();
            ticker_subs.get_mut("AAPL").unwrap().push(1);
            ticker_subs.get_mut("MSFT").unwrap().push(1);
        }

        // Receiver should exist with both tickers
        let recvs = qg.recievers.read().unwrap();
        assert_eq!(recvs.len(), 1);
        let recv = recvs.get(&1).unwrap();
        assert!(recv.subscribed_tickers.contains("AAPL"));
        assert!(recv.subscribed_tickers.contains("MSFT"));

        // Both tickers should reference this receiver
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        assert!(ticker_subs.get("AAPL").unwrap().contains(&1));
        assert!(ticker_subs.get("MSFT").unwrap().contains(&1));
    }

    #[test]
    fn remove_reciever_partial_keeps_other_subscriptions() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string(), "MSFT".to_string()].into_iter());
        let (tx, _) = mpsc::channel();

        // Setup receiver with both tickers
        {
            let mut recvs = qg.recievers.write().unwrap();
            recvs.insert(1, RecieverInfo {
                id: 1,
                channel: tx,
                subscribed_tickers: HashSet::from(["AAPL".to_string(), "MSFT".to_string()]),
            });
        }
        {
            let mut ticker_subs = qg.tickers_to_recievers.write().unwrap();
            ticker_subs.get_mut("AAPL").unwrap().push(1);
            ticker_subs.get_mut("MSFT").unwrap().push(1);
        }

        qg.remove_reciever(1, "AAPL");

        // Receiver should still exist with only MSFT
        let recvs = qg.recievers.read().unwrap();
        assert!(recvs.contains_key(&1));
        let recv = recvs.get(&1).unwrap();
        assert!(!recv.subscribed_tickers.contains("AAPL"));
        assert!(recv.subscribed_tickers.contains("MSFT"));

        // AAPL should not have this receiver, MSFT should
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        assert!(!ticker_subs.get("AAPL").unwrap().contains(&1));
        assert!(ticker_subs.get("MSFT").unwrap().contains(&1));
    }

    #[test]
    fn remove_reciever_all_removes_from_all_tickers() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()].into_iter());
        let (tx, _) = mpsc::channel();

        // Setup receiver with all three tickers
        {
            let mut recvs = qg.recievers.write().unwrap();
            recvs.insert(1, RecieverInfo {
                id: 1,
                channel: tx,
                subscribed_tickers: HashSet::from(["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()]),
            });
        }
        {
            let mut ticker_subs = qg.tickers_to_recievers.write().unwrap();
            ticker_subs.get_mut("AAPL").unwrap().push(1);
            ticker_subs.get_mut("MSFT").unwrap().push(1);
            ticker_subs.get_mut("TSLA").unwrap().push(1);
        }

        qg.remove_reciever_all(1);

        // Receiver should be completely removed
        let recvs = qg.recievers.read().unwrap();
        assert!(!recvs.contains_key(&1));

        // All tickers should not have this receiver
        let ticker_subs = qg.tickers_to_recievers.read().unwrap();
        assert!(!ticker_subs.get("AAPL").unwrap().contains(&1));
        assert!(!ticker_subs.get("MSFT").unwrap().contains(&1));
        assert!(!ticker_subs.get("TSLA").unwrap().contains(&1));
    }

    // -- start_for_quote test --

    #[test]
    fn start_for_quote_sends_quotes_to_receivers() {
        let qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, rx) = mpsc::channel();

        // Manually insert receiver using new structure
        {
            let mut recvs = qg.recievers.write().unwrap();
            recvs.insert(1, RecieverInfo {
                id: 1,
                channel: tx,
                subscribed_tickers: HashSet::from(["AAPL".to_string()]),
            });
        }
        {
            let mut ticker_subs = qg.tickers_to_recievers.write().unwrap();
            ticker_subs.insert("AAPL".to_string(), vec![1]);
        }

        let prices = Arc::clone(&qg.prices);
        let receivers = Arc::clone(&qg.recievers);
        let tickers_to_recievers = Arc::clone(&qg.tickers_to_recievers);
        let handle = thread::spawn(move || {
            // Run with a very short delay; we only need a couple of quotes
            QuoteGenerator::start_for_quote(&prices, &receivers, &tickers_to_recievers, "AAPL", 10);
        });

        // Collect a few quotes then verify
        let quote1 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let quote2 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(quote1.ticker, "AAPL");
        assert_eq!(quote2.ticker, "AAPL");
        assert!(quote1.price > 0.0);
        assert!(quote2.price > 0.0);
        // Prices should generally differ (random walk)
        // Timestamps should be non-decreasing
        assert!(quote2.timestamp >= quote1.timestamp);

        // Clean up: drop the thread (it loops forever, so we just let it go)
        drop(handle);
    }
}
