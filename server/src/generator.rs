use crate::StockQuote;
use crate::errors::GeneratorError;
use core::error;
use rand::Rng;
use std::collections::VecDeque;
use std::hash::Hash;
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

use log::{debug, error, info};

/// Maximum percentage change allowed per price update (2%)
const MAX_CHANGE_PERCENT: f64 = 0.02;

/// Minimum price threshold to prevent prices from going to zero
const MIN_PRICE: f64 = 0.01;

/// Default delay between quote generations in milliseconds
const BASE_DELAY: u64 = 1000;

/// Maximum starting price for randomly initialized tickers
const MAX_START_PRICE: f64 = 100.0;

/// Receiver with channel and subscribed tickers.
pub(crate) struct ReceiverInfo {
    id: u64,
    channel: mpsc::Sender<StockQuote>,
    subscribed_tickers: HashSet<String>,
}

/// Generates new price using random walk (±2% change, minimum MIN_PRICE).
fn generate_new_price(previous_price: f64) -> f64 {
    let mut rng = rand::rng();

    // Generate a random change between -MAX_CHANGE_PERCENT and +MAX_CHANGE_PERCENT
    let change_percent = rng.random_range(-MAX_CHANGE_PERCENT..=MAX_CHANGE_PERCENT);

    // Calculate the new price with proportional change
    let new_price = previous_price * (1.0 + change_percent);

    // Ensure price never goes below minimum
    new_price.max(MIN_PRICE)
}

/// Generates random initial price between MIN_PRICE and MAX_START_PRICE.
fn generate_initial_price() -> f64 {
    let mut rng = rand::rng();
    rng.random_range(MIN_PRICE..MAX_START_PRICE)
}

/// Current price and last update timestamp for a ticker.
pub(crate) struct PriceChange {
    price: f64,
    last_change: u64,
}

/// Quote generator managing price updates and distribution to subscribers.
///
/// Uses three maps for efficient management:
/// - `prices`: ticker -> current price/timestamp
/// - `receivers`: receiver_id -> channel + subscribed tickers
/// - `tickers_to_receivers`: ticker -> [receiver_ids]
///
/// Each ticker runs in its own thread generating quotes. When a quote is generated,
/// it looks up subscribed receivers and sends via their channels.
pub(crate) struct QuoteGenerator {
    pub(crate) prices: Arc<RwLock<HashMap<String, PriceChange>>>,
    pub(crate) receivers: Arc<RwLock<HashMap<u64, ReceiverInfo>>>,
    pub(crate) tickers_to_receivers: Arc<RwLock<HashMap<String, HashSet<u64>>>>,
}

impl QuoteGenerator {
    /// Creates QuoteGenerator with specified tickers initialized at random prices.
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
            tickers_recv_hm.insert(ticker, HashSet::new());
        }

        Self {
            prices: Arc::new(RwLock::new(prices_hm)),
            receivers: Arc::new(RwLock::new(HashMap::new())),
            tickers_to_receivers: Arc::new(RwLock::new(tickers_recv_hm)),
        }
    }

    /// Loads tickers from file (one per line, whitespace trimmed).
    pub(crate) fn new_from_file(input: PathBuf) -> Result<Self, GeneratorError> {
        let file = std::fs::File::open(input)?;
        let reader = BufReader::new(file);
        let tickers = reader
            .lines()
            .collect::<Result<Vec<_>, std::io::Error>>()?
            .into_iter()
            .map(|x| x.trim().to_owned());
        Ok(QuoteGenerator::new(tickers))
    }

    /// Spawns a thread for each ticker to generate and distribute quotes.
    /// Takes Arc parameters to avoid holding mutex lock during execution.
    pub(crate) fn start(
        prices: Arc<RwLock<HashMap<String, PriceChange>>>,
        receivers: Arc<RwLock<HashMap<u64, ReceiverInfo>>>,
        tickers_to_receivers: Arc<RwLock<HashMap<String, HashSet<u64>>>>,
    ) {
        let tickers: Vec<_> = prices.read().unwrap().keys().cloned().collect();
        for ticker in tickers {
            let prices_clone = Arc::clone(&prices);
            let receivers_clone = Arc::clone(&receivers);
            let tickers_to_receivers_clone = Arc::clone(&tickers_to_receivers);
            thread::spawn(move || {
                Self::start_for_quote(
                    &prices_clone,
                    &receivers_clone,
                    &tickers_to_receivers_clone,
                    &ticker,
                    BASE_DELAY,
                );
            });
        }
    }

    /// Generates new quote for ticker with updated price and random volume.
    /// Returns None if ticker doesn't exist.
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
            timestamp,
        })
    }

    /// Main loop for ticker thread: generates quotes and sends to subscribed receivers.
    fn start_for_quote(
        prices: &Arc<RwLock<HashMap<String, PriceChange>>>,
        receivers: &Arc<RwLock<HashMap<u64, ReceiverInfo>>>,
        tickers_to_receivers: &Arc<RwLock<HashMap<String, HashSet<u64>>>>,
        ticker: &str,
        delay: u64,
    ) {
        let mut dead_recv = HashSet::new();
        loop {
            let quote = Self::generate_quote(prices, ticker);
            if let Some(quote) = quote {
                let tickers_to_recv_guard = tickers_to_receivers.read().unwrap();
                let recv_guard = receivers.read().unwrap();
                if let Some(list) = tickers_to_recv_guard.get(ticker) {
                    for receiver_id in list {
                        if let Some(receiver_info) = recv_guard.get(receiver_id) {
                            if receiver_info.channel.send(quote.clone()).is_err() {
                                // Channel closed - receiver disconnected
                                error!(
                                    "Error sending quote to receiver {}: channel closed, cleaning up",
                                    receiver_id
                                );
                                dead_recv.insert(receiver_id.to_owned());
                            }
                        }
                    }
                }
            }
            if dead_recv.len() > 0 {
                debug!("cleaning up {} receivers", dead_recv.len());
                for id in dead_recv.iter() {
                    QuoteGenerator::remove_receiver(
                        *id,
                        &receivers.clone(),
                        &tickers_to_receivers.clone(),
                    );
                }
                dead_recv.clear();
            }
            thread::sleep(Duration::from_millis(delay));
        }
    }

    /// Subscribes receiver to tickers and registers its channel.
    ///
    /// Safety: IDs are generated via `AtomicU64::fetch_add` in `Processor::add_subscriber`,
    /// so duplicate IDs cannot occur. This function unconditionally inserts, overwriting any
    /// stale entry. If ID generation changes to allow reuse, this must be revisited.
    pub(crate) fn add_receiver(
        &mut self,
        id: u64,
        sender: Sender<StockQuote>,
        tickers: Vec<String>,
    ) {
        let tickers_set: HashSet<String> = tickers.iter().cloned().collect();

        self.receivers.write().unwrap().insert(
            id,
            ReceiverInfo {
                id,
                channel: sender,
                subscribed_tickers: tickers_set,
            },
        );

        // 2. Add receiver ID to each ticker's subscription list
        let mut ticker_subs = self.tickers_to_receivers.write().unwrap();
        for ticker in tickers {
            ticker_subs
                .entry(ticker)
                .or_insert(HashSet::new())
                .insert(id);
        }
    }

    /// Unsubscribes receiver from all tickers and removes from registry.
    pub(crate) fn remove_receiver(
        id: u64,
        receivers: &Arc<RwLock<HashMap<u64, ReceiverInfo>>>,
        tickers_to_receivers: &Arc<RwLock<HashMap<String, HashSet<u64>>>>,
    ) {
        // 1. Get all tickers this receiver is subscribed to
        let tickers: Vec<String> = receivers
            .read()
            .unwrap()
            .get(&id)
            .map(|info| info.subscribed_tickers.iter().cloned().collect())
            .unwrap_or_default();

        // 2. Remove receiver from registry
        receivers.write().unwrap().remove(&id);

        // 3. Remove receiver ID from all ticker sets
        let mut ticker_subs = tickers_to_receivers.write().unwrap();
        for ticker in tickers {
            if let Some(ids) = ticker_subs.get_mut(&ticker) {
                ids.remove(&id);
            }
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

    // -- add_receiver / remove_receiver tests --

    #[test]
    fn add_receiver_creates_subscription() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, _rx) = mpsc::channel();
        qg.add_receiver(1, tx, vec!["AAPL".to_string()]);

        // Check receiver was added
        let recvs = qg.receivers.read().unwrap();
        assert!(recvs.contains_key(&1));
        assert_eq!(recvs.get(&1).unwrap().id, 1);
        assert!(recvs.get(&1).unwrap().subscribed_tickers.contains("AAPL"));

        // Check ticker subscription list was updated
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
        assert_eq!(ticker_subs.get("AAPL").unwrap().len(), 1);
        assert!(ticker_subs.get("AAPL").unwrap().contains(&1));
    }

    #[test]
    fn add_multiple_receivers_same_ticker() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        qg.add_receiver(1, tx1, vec!["AAPL".to_string()]);
        qg.add_receiver(2, tx2, vec!["AAPL".to_string()]);

        // Check both receivers exist
        let recvs = qg.receivers.read().unwrap();
        assert_eq!(recvs.len(), 2);
        assert!(recvs.contains_key(&1));
        assert!(recvs.contains_key(&2));

        // Check ticker has both receivers
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
        assert_eq!(ticker_subs.get("AAPL").unwrap().len(), 2);
    }

    #[test]
    fn remove_receiver_removes_correct_subscription() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        qg.add_receiver(1, tx1, vec!["AAPL".to_string()]);
        qg.add_receiver(2, tx2, vec!["AAPL".to_string()]);
        QuoteGenerator::remove_receiver(1, &qg.receivers, &qg.tickers_to_receivers);

        // Receiver 1 should be removed
        let recvs = qg.receivers.read().unwrap();
        assert!(!recvs.contains_key(&1));
        assert!(recvs.contains_key(&2));

        // Ticker should only have receiver 2
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
        let aapl_subs = ticker_subs.get("AAPL").unwrap();
        assert_eq!(aapl_subs.len(), 1);
        assert!(aapl_subs.contains(&2));
    }

    #[test]
    fn remove_receiver_nonexistent_id_does_nothing() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, _) = mpsc::channel();
        qg.add_receiver(1, tx, vec!["AAPL".to_string()]);
        QuoteGenerator::remove_receiver(999, &qg.receivers, &qg.tickers_to_receivers);

        // Receiver 1 should still exist
        let recvs = qg.receivers.read().unwrap();
        assert!(recvs.contains_key(&1));
        assert_eq!(recvs.len(), 1);

        // Ticker should still have receiver 1
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
        assert_eq!(ticker_subs.get("AAPL").unwrap().len(), 1);
    }

    #[test]
    fn add_receiver_multiple_tickers_same_receiver() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string(), "MSFT".to_string()].into_iter());
        let (tx, _) = mpsc::channel();

        // Use add_receiver with multiple tickers
        qg.add_receiver(1, tx, vec!["AAPL".to_string(), "MSFT".to_string()]);

        // Receiver should exist with both tickers
        let recvs = qg.receivers.read().unwrap();
        assert_eq!(recvs.len(), 1);
        let recv = recvs.get(&1).unwrap();
        assert!(recv.subscribed_tickers.contains("AAPL"));
        assert!(recv.subscribed_tickers.contains("MSFT"));

        // Both tickers should reference this receiver
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
        assert!(ticker_subs.get("AAPL").unwrap().contains(&1));
        assert!(ticker_subs.get("MSFT").unwrap().contains(&1));
    }

    #[test]
    fn remove_receiver_removes_from_all_subscriptions() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string(), "MSFT".to_string()].into_iter());
        let (tx, _) = mpsc::channel();

        // Add receiver with both tickers
        qg.add_receiver(1, tx, vec!["AAPL".to_string(), "MSFT".to_string()]);

        QuoteGenerator::remove_receiver(1, &qg.receivers, &qg.tickers_to_receivers);

        // Receiver should be completely removed
        let recvs = qg.receivers.read().unwrap();
        assert!(!recvs.contains_key(&1));

        // Neither ticker should have this receiver
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
        assert!(!ticker_subs.get("AAPL").unwrap().contains(&1));
        assert!(!ticker_subs.get("MSFT").unwrap().contains(&1));
    }

    #[test]
    fn remove_receiver_removes_from_multiple_tickers() {
        let mut qg = QuoteGenerator::new(
            vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()].into_iter(),
        );
        let (tx, _) = mpsc::channel();

        // Add receiver with all three tickers
        qg.add_receiver(
            1,
            tx,
            vec!["AAPL".to_string(), "MSFT".to_string(), "TSLA".to_string()],
        );

        QuoteGenerator::remove_receiver(1, &qg.receivers, &qg.tickers_to_receivers);

        // Receiver should be completely removed
        let recvs = qg.receivers.read().unwrap();
        assert!(!recvs.contains_key(&1));

        // All tickers should not have this receiver
        let ticker_subs = qg.tickers_to_receivers.read().unwrap();
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
            let mut recvs = qg.receivers.write().unwrap();
            recvs.insert(
                1,
                ReceiverInfo {
                    id: 1,
                    channel: tx,
                    subscribed_tickers: HashSet::from(["AAPL".to_string()]),
                },
            );
        }
        {
            let mut ticker_subs = qg.tickers_to_receivers.write().unwrap();
            ticker_subs.insert("AAPL".to_string(), HashSet::from([1]));
        }

        let prices = Arc::clone(&qg.prices);
        let receivers = Arc::clone(&qg.receivers);
        let tickers_to_receivers = Arc::clone(&qg.tickers_to_receivers);
        let handle = thread::spawn(move || {
            // Run with a very short delay; we only need a couple of quotes
            QuoteGenerator::start_for_quote(&prices, &receivers, &tickers_to_receivers, "AAPL", 10);
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
