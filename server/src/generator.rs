use crate::StockQuote;
use rand::Rng;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock, mpsc::Sender},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_CHANGE_PERCENT: f64 = 0.02; // Maximum 2% change per tick
const MIN_PRICE: f64 = 0.01; // Minimum price to prevent going to zero

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
    last_change: u64,
}

pub(crate) struct QuoteGenerator {
    pub(crate) prices: Arc<RwLock<HashMap<String, PriceChange>>>,
}

impl QuoteGenerator {
    pub(crate) fn generate_quote(&mut self, ticker: &str) -> Option<StockQuote> {
        let new_price = {
            let prices = self.prices.read().unwrap();
            let last_data = prices.get(ticker)?;
            generate_new_price(last_data.price)
        };

        // Update stored price
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        {
            let mut prices = self.prices.write().unwrap();
            if let Some(data) = prices.get_mut(ticker) {
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
}
