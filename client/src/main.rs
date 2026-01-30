mod errors;
mod worker;

use clap::Parser;
use worker::Worker;

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

    let worker = Worker::new(
        args.master_addr,
        args.master_tcp_port,
        args.worker_udp_port,
    );

    worker.start(args.tickers);
}
