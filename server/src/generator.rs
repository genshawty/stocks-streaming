use crate::StockQuote;
use rand::Rng;
use std::ops::Sub;
use std::thread;
use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock, mpsc::Sender},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_CHANGE_PERCENT: f64 = 0.02; // Maximum 2% change per tick
const MIN_PRICE: f64 = 0.01; // Minimum price to prevent going to zero
const BASE_DELAY: u64 = 1000;

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

    pub(crate) fn add_reciever(&mut self, id: u64, sender: Sender<StockQuote>, ticker: &str) {
        let subscription = Subscription { id, sender };
        self.recievers
            .write()
            .unwrap()
            .entry(ticker.to_owned())
            .and_modify(|x| x.push(subscription.clone()))
            .or_insert(vec![subscription]);
    }

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
