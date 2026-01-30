#![warn(missing_docs)]

pub mod errors;
mod generator;
pub mod processor;
pub mod types;

pub use types::StockQuote;

fn main() {
    println!("Hello, world!");
}
