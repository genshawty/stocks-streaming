mod errors;
mod worker;

use clap::Parser;
use worker::Worker;

use env_logger::{Builder, Env};
use log::info;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MASTER_TCP_PORT: u16 = 8090;
const WORKER_UDP_PORT: u16 = 8091;

#[derive(Parser, Debug)]
#[command(name = "trading-stream-client")]
#[command(about = "Stock trading stream worker client", long_about = None)]
struct Args {
    /// Master server address
    #[arg(short, long, alias = "server-addr", default_value = "127.0.0.1")]
    master_addr: String,

    /// Master TCP port
    #[arg(short = 'p', long, alias = "tcp-port", default_value_t = MASTER_TCP_PORT)]
    master_tcp_port: u16,

    /// Worker UDP port
    #[arg(short = 'u', long, alias = "udp-port", default_value_t = WORKER_UDP_PORT)]
    worker_udp_port: u16,

    /// Stock tickers to subscribe to (comma-separated)
    #[arg(short, long, value_delimiter = ',')]
    tickers: Vec<String>,

    /// Path to a text file with tickers (one per line)
    #[arg(short = 'f', long, alias = "tickers-file")]
    tickers_file: Option<std::path::PathBuf>,
}

fn main() {
    let env = Env::new().filter_or("RUST_LOG", "info");
    Builder::from_env(env).init();
    let args = Args::parse();

    let tickers = if let Some(ref path) = args.tickers_file {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read tickers file {:?}: {}", path, e));
        content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    } else if !args.tickers.is_empty() {
        args.tickers
    } else {
        panic!("No tickers has been specified")
    };

    info!("Starting worker with:");
    info!("  Master: {}:{}", args.master_addr, args.master_tcp_port);
    info!("  UDP Port: {}", args.worker_udp_port);
    info!("  Tickers: {:?}", tickers);

    let worker = Worker::new(args.master_addr, args.master_tcp_port, args.worker_udp_port)
        .expect("worker creation error");

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let handle = worker.start(tickers);

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    while running.load(Ordering::SeqCst) && !*worker.shutdown.read().unwrap() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    info!("Shutting down...");
    *worker.shutdown.write().unwrap() = true;

    let res = handle.join().expect("could not join handle");
    match res {
        Ok(()) => {
            info!("Shutdown complete")
        }
        Err(e) => {
            info!("Shutdown complete with error {e}")
        }
    }
}
