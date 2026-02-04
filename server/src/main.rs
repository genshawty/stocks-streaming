#![warn(missing_docs)]
//! Stock quote streaming server
//!
//! This server generates random stock quotes and streams them to subscribed clients via UDP.
//! Clients connect via TCP to subscribe to specific stock tickers.

use clap::Parser;
use env_logger::{Builder, Env};
use log::{error, info};
use processor::Processor;
use std::path::PathBuf;

/// Error types for the server.
pub mod errors;
/// Stock quote generator.
pub mod generator;
/// TCP server and subscriber management.
pub mod processor;
/// Data types for quotes, commands, and messages.
pub mod types;
pub mod utils;

pub use types::StockQuote;

/// Default TCP port for the master server
pub const MASTER_TCP_PORT: u16 = 8090;

/// Stock quote streaming server
#[derive(Parser, Debug)]
#[command(name = "server")]
#[command(about = "Stock quote streaming server", long_about = None)]
struct Args {
    /// TCP port to listen on
    #[arg(short, long, default_value_t = MASTER_TCP_PORT)]
    port: u16,

    /// Path to tickers file (one ticker per line)
    #[arg(short, long, default_value = "src/data/tickers.txt")]
    tickers_file: PathBuf,
}

fn main() {
    let env = Env::new().filter_or("RUST_LOG", "info");
    Builder::from_env(env).init();
    let args = Args::parse();
    info!(
        "Starting stock quote streaming server on port {}",
        args.port
    );
    info!("Loading tickers from: {:?}", args.tickers_file);

    // Create and start the processor
    let processor = match Processor::new(args.port, args.tickers_file) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to initialize server: {}", e);
            std::process::exit(1);
        }
    };

    // Start the TCP server (this blocks forever)
    if let Err(e) = processor.start_tcp_server() {
        error!("Server error: {}", e);
        std::process::exit(1);
    }
}
