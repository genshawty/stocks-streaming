//! Trading stream server library.
//!
//! Provides types and errors for streaming stock market quotes over TCP/UDP.

pub mod errors;
// pub mod generator;
// pub mod processor;
pub mod types;
pub mod utils;

pub use types::{Commands, Protocol, StockQuote, SubscribeCommand, TcpMessage, UdpMessage};
