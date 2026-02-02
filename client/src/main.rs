mod errors;
mod worker;

use clap::Parser;
use worker::Worker;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MASTER_TCP_PORT: u16 = 8090;
const WORKER_UDP_PORT: u16 = 8091;

#[derive(Parser, Debug)]
#[command(name = "trading-stream-client")]
#[command(about = "Stock trading stream worker client", long_about = None)]
struct Args {
    /// Master server address
    #[arg(short, long, default_value = "127.0.0.1")]
    master_addr: String,

    /// Master TCP port
    #[arg(short = 'p', long, default_value_t = MASTER_TCP_PORT)]
    master_tcp_port: u16,

    /// Worker UDP port
    #[arg(short = 'u', long, default_value_t = WORKER_UDP_PORT)]
    worker_udp_port: u16,

    /// Stock tickers to subscribe to (comma-separated)
    #[arg(short, long, value_delimiter = ',', default_value = "AAPL")]
    tickers: Vec<String>,
}

fn main() {
    let args = Args::parse();

    println!("Starting worker with:");
    println!("  Master: {}:{}", args.master_addr, args.master_tcp_port);
    println!("  UDP Port: {}", args.worker_udp_port);
    println!("  Tickers: {:?}", args.tickers);

    let worker = Worker::new(args.master_addr, args.master_tcp_port, args.worker_udp_port);

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    let handle = worker.start(args.tickers);

    // Wait for Ctrl+C, sleeping to avoid busy-wait
    while running.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    println!("Shutting down...");
    *worker.shutdown.write().unwrap() = true;

    let _ = handle.join();
    println!("Shutdown complete");
}
