use crate::errors::WorkerError;
use server::{Commands, Protocol, StockQuote, SubscribeCommand};
use std::io::Write;
use std::net::{Ipv4Addr, TcpStream, UdpSocket};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

const MAX_DATAGRAM_SIZE: usize = 65536;

pub struct Worker {
    pub master_addr: String,
    pub master_tcp_port: u16,
    pub worker_udp_port: u16,

    shutdown: Arc<RwLock<bool>>,
}

impl Worker {
    pub fn new(master_addr: String, master_tcp_port: u16, worker_udp_port: u16) -> Self {
        Self {
            master_addr,
            master_tcp_port,
            worker_udp_port,

            shutdown: Arc::new(RwLock::new(false)),
        }
    }

    pub fn start(&self, tickers: Vec<String>) {
        println!("Worker starting...");
        let shutdown_clone = self.shutdown.clone();
        let worker_udp_port = self.worker_udp_port;
        let handle =
            std::thread::spawn(move || Worker::udp_listener_loop(shutdown_clone, worker_udp_port));

        loop {
            match TcpStream::connect(format!("{}:{}", self.master_addr, self.master_tcp_port)) {
                Ok(stream) => {
                    println!(
                        "Connected to master at {}:{}",
                        self.master_addr, self.master_tcp_port
                    );

                    if let Err(e) = self.send_command(stream, tickers.clone()) {
                        eprintln!("Work error: {}", e);
                        println!("Reconnecting in 3 seconds...");
                        std::thread::sleep(Duration::from_secs(3));
                    } else {
                        thread::sleep(Duration::from_secs(10));
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Failed to connect: {}. Retrying in 5 seconds...", e);
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        }
        handle.join();
    }

    fn send_command(&self, mut stream: TcpStream, tickers: Vec<String>) -> Result<(), WorkerError> {
        let cmd = SubscribeCommand {
            cmd: Commands::Stream,
            protocol: Protocol::Udp,
            ip: Ipv4Addr::new(127, 0, 0, 1),
            port: self.worker_udp_port,
            tickers_list: tickers,
        }
        .to_string();
        let cmd_bytes = cmd.as_bytes();
        let data_len = (cmd_bytes.len() as u32).to_be_bytes();

        stream.write_all(&data_len)?;
        stream.write_all(cmd_bytes)?;

        println!("Sent subscribe command to master");
        Ok(())
    }

    fn udp_listener_loop(
        shutdown: Arc<RwLock<bool>>,
        worker_udp_port: u16,
    ) -> Result<(), WorkerError> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", worker_udp_port))?;
        println!("UDP listener started on port {}", worker_udp_port);

        socket.set_read_timeout(Some(Duration::from_millis(100)))?;

        // Buffer large enough for max UDP packet size
        let mut buf = [0u8; MAX_DATAGRAM_SIZE];

        loop {
            if *shutdown.read().unwrap() {
                println!("UDP listener shutting down");
                break;
            }

            // Receive the entire UDP datagram (header + data in one packet)
            match socket.recv_from(&mut buf) {
                Ok((size, _src_addr)) => {
                    // Minimum packet size is 4 bytes (length header)
                    if size < 4 {
                        eprintln!("Wrong size header: {}", size);
                        continue;
                    }

                    // Parse the length header from first 4 bytes
                    let data_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

                    // Deserialize StockQuote from the data portion (after the 4-byte header)
                    match StockQuote::from_bytes(buf[4..4 + data_len].to_vec()) {
                        Ok(quote) => {
                            println!("{:?}", quote);
                        }
                        Err(e) => {
                            eprintln!("Failed to deserialize stock quote: {}", e);
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(ref e) if e.raw_os_error() == Some(35) => continue,
                Err(e) => {
                    eprintln!("UDP receive error: {}", e);
                }
            }
        }
        Ok(())
    }
}
