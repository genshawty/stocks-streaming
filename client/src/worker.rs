use crate::errors::WorkerError;
use log::{error, info};
use server::{Commands, Protocol, SubscribeCommand, UdpMessage};
use std::collections::VecDeque;
use std::io::Write;
use std::net::{AddrParseError, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_DATAGRAM_SIZE: usize = 65536;
const MAX_PING_LOGS: usize = 100;
const BASE_PING_DELAY: u64 = 2;

pub struct PingRecord {
    timestamp: u64,
    rtt_ms: Option<u64>,
}

pub struct Worker {
    pub master_addr: Ipv4Addr,
    pub master_tcp_port: u16,
    pub worker_udp_port: u16,

    pub shutdown: Arc<RwLock<bool>>,
    pub pings: Arc<RwLock<VecDeque<PingRecord>>>,
}

impl Worker {
    pub fn new(
        master_addr: String,
        master_tcp_port: u16,
        worker_udp_port: u16,
    ) -> Result<Self, AddrParseError> {
        let master_addr = Ipv4Addr::from_str(&master_addr)?;
        Ok(Self {
            master_addr,
            master_tcp_port,
            worker_udp_port,

            shutdown: Arc::new(RwLock::new(false)),
            pings: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_PING_LOGS))),
        })
    }

    pub fn start(&self, tickers: Vec<String>) -> JoinHandle<Result<(), WorkerError>> {
        info!("Worker starting...");
        let shutdown_clone = self.shutdown.clone();
        let worker_udp_port = self.worker_udp_port;
        let pings_clone = self.pings.clone();
        let handle = std::thread::spawn(move || {
            Worker::udp_listener_loop(pings_clone, shutdown_clone, worker_udp_port)
        });

        loop {
            match TcpStream::connect(format!("{}:{}", self.master_addr, self.master_tcp_port)) {
                Ok(stream) => {
                    info!(
                        "Connected to master at {}:{}",
                        self.master_addr, self.master_tcp_port
                    );

                    if let Err(e) = self.send_command(stream, tickers.clone()) {
                        error!("Work error: {}", e);
                        info!("Reconnecting in 3 seconds...");
                        std::thread::sleep(Duration::from_secs(3));
                    } else {
                        info!("Successfully subscribed, receiving quotes...");
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to connect: {}. Retrying in 5 seconds...", e);
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        }
        handle
    }

    fn send_command(&self, mut stream: TcpStream, tickers: Vec<String>) -> Result<(), WorkerError> {
        let cmd = SubscribeCommand {
            cmd: Commands::Stream,
            protocol: Protocol::Udp,
            ip: self.master_addr,
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
        pings: Arc<RwLock<VecDeque<PingRecord>>>,
        shutdown: Arc<RwLock<bool>>,
        worker_udp_port: u16,
    ) -> Result<(), WorkerError> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", worker_udp_port))?;
        println!("UDP listener started on port {}", worker_udp_port);

        socket.set_read_timeout(Some(Duration::from_millis(100)))?;

        let mut buf = [0u8; MAX_DATAGRAM_SIZE];
        let mut src_addr = None;
        let mut ping_handle: Option<JoinHandle<Result<(), WorkerError>>> = None;

        loop {
            if *shutdown.read().unwrap() {
                println!("UDP listener shutting down");
                break;
            }

            match socket.recv_from(&mut buf) {
                Ok((size, _src_addr)) => match UdpMessage::from_bytes(buf[..size].to_vec()) {
                    Ok(msg) => {
                        if src_addr.is_none() {
                            let socket_clone = socket.try_clone().expect("could not clone socket");
                            src_addr = Some(_src_addr);

                            let pings_clone = pings.clone();
                            let shutdown_clone = shutdown.clone();

                            ping_handle = Some(thread::spawn(move || {
                                Worker::start_pinging(
                                    socket_clone,
                                    _src_addr,
                                    pings_clone,
                                    shutdown_clone,
                                )
                            }));
                        }
                        match msg {
                            UdpMessage::Quote(quote) => {
                                println!("{:?}", quote);
                            }
                            UdpMessage::Pong { timestamp } => {
                                let curr_timestamp = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_millis()
                                    as u64;

                                // matching pong with already sent ping
                                let mut pings_guard = pings.write().unwrap();
                                if let Some(pos) =
                                    pings_guard.iter().position(|x| x.timestamp == timestamp)
                                {
                                    pings_guard[pos].rtt_ms =
                                        Some(curr_timestamp.saturating_sub(timestamp));
                                }
                            }
                            _ => {
                                eprintln!("unexpected message: {:?}", msg)
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to deserialize stock quote: {}", e);
                    }
                },
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(ref e) if e.raw_os_error() == Some(35) => continue,
                Err(e) => {
                    eprintln!("UDP receive error: {}", e);
                }
            }
        }
        if let Some(handle) = ping_handle {
            match handle.join() {
                Ok(res) => {
                    res?;
                }
                Err(_panic) => {
                    eprintln!("ping thread panicked")
                }
            }
        }
        Ok(())
    }

    fn start_pinging(
        socket: UdpSocket,
        src_addr: SocketAddr,
        pings: Arc<RwLock<VecDeque<PingRecord>>>,
        shutdown: Arc<RwLock<bool>>,
    ) -> Result<(), WorkerError> {
        loop {
            if *shutdown.read().unwrap() {
                println!("Shutting down pings");
                break;
            }
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            // here i believe never be a problem to serialize enum, since it covered with tests already
            match socket.send_to(
                &UdpMessage::Ping { timestamp }
                    .to_bytes()
                    .expect("err serializing ping message"),
                src_addr,
            ) {
                Ok(_bytes_sent) => {
                    let mut pings_guard = pings.write().unwrap();
                    if pings_guard.len() >= MAX_PING_LOGS {
                        pings_guard.pop_front();
                    }
                    pings_guard.push_back(PingRecord {
                        timestamp,
                        rtt_ms: None,
                    });
                }
                Err(e) => {
                    eprintln!("err sending ping: {}", e);
                }
            }
            std::thread::sleep(Duration::from_secs(BASE_PING_DELAY));
        }
        Ok(())
    }
}
