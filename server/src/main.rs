//! Stock quote streaming server
//!
//! This server generates random stock quotes and streams them to subscribed clients via UDP.
//! Clients connect via TCP to subscribe to specific stock tickers.

use clap::Parser;
use processor::Processor;
use std::path::PathBuf;

pub mod errors;
mod generator;
pub mod processor;
pub mod types;

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
    let args = Args::parse();
    println!(
        "Starting stock quote streaming server on port {}",
        args.port
    );
    println!("Loading tickers from: {:?}", args.tickers_file);

    // Create and start the processor
    let processor = match Processor::new(args.port, args.tickers_file) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to initialize server: {}", e);
            std::process::exit(1);
        }
    };

    // Start the TCP server (this blocks forever)
    if let Err(e) = processor.start_tcp_server() {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }
}
