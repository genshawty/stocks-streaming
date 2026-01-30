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

const MAX_CHANGE_PERCENT: f64 = 0.02; // Maximum 2% change per tick
const MIN_PRICE: f64 = 0.01; // Minimum price to prevent going to zero
const BASE_DELAY: u64 = 1000;

const MAX_START_PRICE: f64 = 100.0;

struct RecieverInfo {
    id: u64,
    channel: mpsc::Sender<StockQuote>,
    subscribed_tickers: HashSet<String>,
}

/// Generates a new price using random walk algorithm.
/// Changes are proportional to the current price and bounded to prevent negative values.
fn generate_new_price(previous_price: f64) -> f64 {
    let mut rng = rand::rng();

    // Generate a random change between -MAX_CHANGE_PERCENT and +MAX_CHANGE_PERCENT
    let change_percent = rng.random_range(-MAX_CHANGE_PERCENT..=MAX_CHANGE_PERCENT);

    // Calculate the new price with proportional change
    let new_price = previous_price * (1.0 + change_percent);

    // Ensure price never goes below minimum
    new_price.max(MIN_PRICE)
}

fn generate_initial_price() -> f64 {
    let mut rng = rand::rng();
    rng.random_range(MIN_PRICE..MAX_START_PRICE)
}

struct PriceChange {
    price: f64,

    // in case if we want to calculate delay between new changes depending on time of last change
    last_change: u64,
}

#[derive(Clone)]
struct Subscription {
    id: u64,
    sender: Sender<StockQuote>,
}

pub(crate) struct QuoteGenerator {
    pub(crate) prices: Arc<RwLock<HashMap<String, PriceChange>>>,
    pub(crate) recievers: Arc<RwLock<HashMap<String, Vec<Subscription>>>>,
}

impl QuoteGenerator {
    // creates new instance of generator, with empty list of tickers
    pub(crate) fn new<T>(tickers: T) -> Self
    where
        T: Iterator<Item = String>,
    {
        let mut hm = HashMap::new();
        for ticker in tickers {
            hm.insert(
                ticker,
                PriceChange {
                    price: generate_initial_price(),
                    last_change: 0u64,
                },
            );
        }
        let prices = Arc::new(RwLock::new(hm));
        let recievers = Arc::new(RwLock::new(HashMap::new()));
        Self {
            prices: prices,
            recievers: recievers,
        }
    }

    // creates new instance of generator, with empty list of tickers
    pub(crate) fn new_from_file(input: PathBuf) -> Result<Self, GeneratorError> {
        let file = std::fs::File::open(input)?;
        let reader = BufReader::new(file);
        let tickers = reader.lines().map(|x| x.unwrap().trim().to_owned());
        Ok(QuoteGenerator::new(tickers))
    }

    // starts generator running
    // new recievers can be added with specific methods during runtime
    pub(crate) fn start(&self) {
        let tickers: Vec<_> = self.prices.read().unwrap().keys().cloned().collect();
        thread::scope(|scope| {
            for ticker in tickers {
                let prices = Arc::clone(&self.prices);
                let receivers = Arc::clone(&self.recievers);
                scope.spawn(move || {
                    Self::start_for_quote(&prices, &receivers, &ticker, BASE_DELAY);
                });
            }
        })
    }

    // generates new quote for specific ticker AND updates prices!!!
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

    // starts generating new prices and sending this to the recievers for specific quote
    // delay should be specified since we price changings to be discrete
    fn start_for_quote(
        prices: &Arc<RwLock<HashMap<String, PriceChange>>>,
        receivers: &Arc<RwLock<HashMap<String, Vec<Subscription>>>>,
        ticker: &str,
        delay: u64,
    ) {
        loop {
            let quote = Self::generate_quote(prices, ticker);
            if let Some(quote) = quote {
                let receivers_guard = receivers.read().unwrap();
                if let Some(list) = receivers_guard.get(ticker) {
                    for sender in list {
                        if sender.sender.send(quote.clone()).is_err() {
                            println!("error sending quote via channel")
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(delay));
        }
    }

    // add new reciever which will be getting the updates about quotes on certain ticker
    pub(crate) fn add_reciever(&mut self, id: u64, sender: Sender<StockQuote>, ticker: &str) {
        let subscription = Subscription { id, sender };
        self.recievers
            .write()
            .unwrap()
            .entry(ticker.to_owned())
            .and_modify(|x| x.push(subscription.clone()))
            .or_insert(vec![subscription]);
    }

    // remove existing reciever
    pub(crate) fn remove_reciever(&mut self, id: u64, ticker: &str) {
        // TODO: add here something to handle if no reciever with certain id has been found
        self.recievers
            .write()
            .unwrap()
            .entry(ticker.to_owned())
            .and_modify(|x| {
                if let Some(index) = x.iter().position(|x| x.id == id) {
                    x.remove(index);
                }
            });
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

        let recvs = qg.recievers.read().unwrap();
        let subs = recvs.get("AAPL").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, 1);
    }

    #[test]
    fn add_multiple_recievers_same_ticker() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        qg.add_reciever(1, tx1, "AAPL");
        qg.add_reciever(2, tx2, "AAPL");

        let recvs = qg.recievers.read().unwrap();
        assert_eq!(recvs.get("AAPL").unwrap().len(), 2);
    }

    #[test]
    fn remove_reciever_removes_correct_subscription() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx1, _) = mpsc::channel();
        let (tx2, _) = mpsc::channel();
        qg.add_reciever(1, tx1, "AAPL");
        qg.add_reciever(2, tx2, "AAPL");
        qg.remove_reciever(1, "AAPL");

        let recvs = qg.recievers.read().unwrap();
        let subs = recvs.get("AAPL").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, 2);
    }

    #[test]
    fn remove_reciever_nonexistent_id_does_nothing() {
        let mut qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, _) = mpsc::channel();
        qg.add_reciever(1, tx, "AAPL");
        qg.remove_reciever(999, "AAPL");

        let recvs = qg.recievers.read().unwrap();
        assert_eq!(recvs.get("AAPL").unwrap().len(), 1);
    }

    // -- start_for_quote test --

    #[test]
    fn start_for_quote_sends_quotes_to_receivers() {
        let qg = QuoteGenerator::new(vec!["AAPL".to_string()].into_iter());
        let (tx, rx) = mpsc::channel();
        {
            let mut recvs = qg.recievers.write().unwrap();
            recvs.insert("AAPL".to_string(), vec![Subscription { id: 1, sender: tx }]);
        }

        let prices = Arc::clone(&qg.prices);
        let receivers = Arc::clone(&qg.recievers);
        let handle = thread::spawn(move || {
            // Run with a very short delay; we only need a couple of quotes
            QuoteGenerator::start_for_quote(&prices, &receivers, "AAPL", 10);
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
