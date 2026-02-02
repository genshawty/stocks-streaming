use log::{debug, error, info};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::ops::Sub;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::errors::ProcessorError;
use crate::generator::QuoteGenerator;
use crate::types::{StockQuote, SubscribeCommand, TcpMessage, UdpMessage};

const MAX_PING_TIMEOUT: u64 = 5;

/// Handle to a subscriber's streaming thread.
struct SubscriberHandle {
    id: u64,
    thread_handle: JoinHandle<()>,
    shutdown: Arc<AtomicBool>,
    tickers: Vec<String>,
}

/// Information passed to streaming thread.
struct SubscriberInfo {
    id: u64,
    channel: Receiver<StockQuote>,
    tickers: Vec<String>,
    udp_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
}

pub(crate) struct Processor {
    port: u16,
    generator: Arc<Mutex<QuoteGenerator>>,
    subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
    next_id: Arc<AtomicU64>,
    shutdown: Arc<RwLock<bool>>,
}

impl Processor {
    /// Creates a new Processor with the specified port and tickers file.
    pub(crate) fn new(port: u16, tickers_file: std::path::PathBuf) -> Result<Self, ProcessorError> {
        // Load generator from tickers file
        let generator = QuoteGenerator::new_from_file(tickers_file)?;

        Ok(Self {
            port,
            generator: Arc::new(Mutex::new(generator)),
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            shutdown: Arc::new(RwLock::new(false)),
        })
    }

    pub(crate) fn start_tcp_server(&self) -> Result<(), ProcessorError> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port))?;
        info!("TCP server started on port {}", self.port);
        listener.set_nonblocking(true)?;

        // Clone Arc parameters without holding the mutex lock
        let (prices, receivers, tickers_to_receivers, gen_shutdown) = {
            let generator_guard = self.generator.lock().unwrap();
            (
                Arc::clone(&generator_guard.prices),
                Arc::clone(&generator_guard.receivers),
                Arc::clone(&generator_guard.tickers_to_receivers),
                Arc::clone(&generator_guard.shutdown),
            )
        }; // Lock is released here

        // Start generator threads
        thread::spawn(|| {
            QuoteGenerator::start(prices, receivers, tickers_to_receivers, gen_shutdown)
        });

        loop {
            if *self.shutdown.read().unwrap() {
                info!("TCP server shutting down");
                break;
            }

            match listener.accept() {
                Ok((stream, addr)) => {
                    info!("New worker connected: {}", addr);
                    let gen_clone = self.generator.clone();
                    let subscribers_clone = self.subscribers.clone();
                    let next_clone = self.next_id.clone();

                    thread::spawn(move || {
                        if let Err(e) =
                            Self::handle_request(stream, gen_clone, subscribers_clone, next_clone)
                        {
                            error!("Worker handler error: {}", e);
                        }
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => error!("Accept TCP error: {}", e),
            }
        }

        Ok(())
    }

    fn handle_request(
        mut stream: TcpStream,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
        next_id: Arc<AtomicU64>,
    ) -> Result<(), ProcessorError> {
        let msg = Self::read_tcp_message(&mut stream)?;

        let response = match msg {
            TcpMessage::Cmd(cmd) => {
                match Self::add_subscriber(cmd, generator, subscribers, next_id) {
                    Ok(_) => {
                        info!("Subscriber added successfully, sending ACK");
                        TcpMessage::Ack
                    }
                    Err(e) => {
                        error!("Failed to add subscriber: {}", e);
                        TcpMessage::Err(format!("{}", e))
                    }
                }
            }
            _ => {
                error!("Unexpected TCP message type (expected Cmd)");
                TcpMessage::Err("Unexpected message type".to_string())
            }
        };

        Self::send_tcp_message(&mut stream, &response)?;
        Ok(())
    }

    fn read_tcp_message(stream: &mut TcpStream) -> Result<TcpMessage, ProcessorError> {
        // Read 4-byte message length prefix
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                error!("Client disconnected while reading message length");
            } else {
                error!("Error reading message length: {}", e);
            }
            ProcessorError::from(e)
        })?;

        // Read message data
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        let mut data = vec![0u8; msg_len];
        stream.read_exact(&mut data).map_err(|e| {
            error!("Error reading message data (length: {}): {}", msg_len, e);
            ProcessorError::from(e)
        })?;

        // Parse UTF-8 string
        let msg_str = std::str::from_utf8(&data).map_err(|e| {
            error!("Invalid UTF-8 in message data: {}", e);
            ProcessorError::ParseErr(e.into())
        })?;

        // Parse TcpMessage
        TcpMessage::from_string(msg_str).map_err(|e| {
            error!("Failed to parse TCP message '{}': {}", msg_str, e);
            e.into()
        })
    }

    fn send_tcp_message(stream: &mut TcpStream, msg: &TcpMessage) -> Result<(), ProcessorError> {
        let msg_str = msg.to_string();
        let msg_bytes = msg_str.as_bytes();
        let len = msg_bytes.len() as u32;

        // Write 4-byte length prefix
        stream.write_all(&len.to_be_bytes()).map_err(|e| {
            error!("Error writing message length: {}", e);
            ProcessorError::from(e)
        })?;

        // Write message data
        stream.write_all(msg_bytes).map_err(|e| {
            error!("Error writing message data: {}", e);
            ProcessorError::from(e)
        })?;

        info!("Sent TCP message: {}", msg_str);
        Ok(())
    }

    /// Adds a new subscriber and starts streaming thread.
    fn add_subscriber(
        cmd: SubscribeCommand,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
        next_id: Arc<AtomicU64>,
    ) -> Result<u64, ProcessorError> {
        // 1. Generate unique ID
        let id = next_id.fetch_add(1, Ordering::Relaxed);

        // 2. Create shutdown signal (shared between handle and thread)
        let shutdown = Arc::new(AtomicBool::new(false));

        // 3. Create channel for receiving quotes from generator
        let (tx, rx) = mpsc::channel();

        // 4. Register with generator for all tickers
        {
            let mut generator_lock = generator.lock().unwrap();
            generator_lock.add_receiver(id, tx.clone(), cmd.tickers_list.clone())?;
        }

        // 5. Create subscriber info for streaming thread
        let sub_info = SubscriberInfo {
            id,
            channel: rx,
            tickers: cmd.tickers_list.clone(),
            udp_addr: SocketAddr::new(cmd.ip.into(), cmd.port),
            shutdown: Arc::clone(&shutdown),
        };
        let subscribers_clone = subscribers.clone();
        // 6. Spawn streaming thread
        let handle = thread::spawn(move || {
            Self::start_streaming(sub_info, generator, subscribers_clone);
        });

        // 7. Store handle
        subscribers.write().unwrap().insert(
            id,
            SubscriberHandle {
                id,
                thread_handle: handle,
                shutdown: Arc::clone(&shutdown),
                tickers: cmd.tickers_list.clone(),
            },
        );

        println!(
            "Added subscriber {} for tickers: {:?}",
            id, cmd.tickers_list
        );
        Ok(id)
    }

    /// Removes subscriber and cleans up resources.
    fn remove_subscriber(
        id: u64,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
    ) {
        // 1. Remove from subscribers map
        let handle = subscribers.write().unwrap().remove(&id);

        if let Some(handle) = handle {
            println!("Removing subscriber {}", id);

            // 2. Signal shutdown
            handle.shutdown.store(true, Ordering::Relaxed);

            // 3. Remove from generator
            let (recv, tickers_to_recv) = {
                let generator_lock = generator.lock().unwrap();
                (
                    generator_lock.receivers.clone(),
                    generator_lock.tickers_to_receivers.clone(),
                )
            };

            QuoteGenerator::remove_receiver(id, &recv, &tickers_to_recv);

            println!("Subscriber {} removed", id);
        }
    }

    /// Streaming loop that receives quotes and sends via UDP.
    fn start_streaming(
        info: SubscriberInfo,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
    ) {
        // Create UDP socket for this subscriber
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Failed to create UDP socket for subscriber {}: {}",
                    info.id, e
                );
                return;
            }
        };

        println!(
            "Started streaming for subscriber {} to {}",
            info.id, info.udp_addr
        );
        let socket_clone = socket.try_clone().expect("err cloning socket");
        let shutdown_clone = info.shutdown.clone();

        thread::spawn(move || Processor::monitor_connection(socket_clone, shutdown_clone));

        loop {
            // Check shutdown signal
            if info.shutdown.load(Ordering::Relaxed) {
                println!("Subscriber {} received shutdown signal", info.id);

                break;
            }

            // Receive quote with timeout (non-blocking)
            match info.channel.recv_timeout(Duration::from_millis(100)) {
                Ok(quote) => {
                    let msg = UdpMessage::Quote(quote);
                    // Serialize quote to bytes
                    match msg.to_bytes() {
                        Ok(bytes) => {
                            println!("sending quote");
                            // Send as single UDP datagram
                            if let Err(e) = socket.send_to(&bytes, info.udp_addr) {
                                eprintln!("UDP send failed for subscriber {}: {}", info.id, e);
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Failed to serialize quote for subscriber {}: {}",
                                info.id, e
                            );
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No quote received, continue
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    println!("Channel disconnected for subscriber {}", info.id);
                    break;
                }
            }
        }
        Processor::remove_subscriber(info.id, generator, subscribers);
        println!("Streaming ended for subscriber {}", info.id);
    }

    fn monitor_connection(
        socket: UdpSocket,
        shutdown: Arc<AtomicBool>,
    ) -> Result<(), ProcessorError> {
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("err setting read timeout");
        // to monitor last pings and consider dead after 15 secs
        let mut last_ping = Instant::now();
        let timeout = Duration::from_secs(MAX_PING_TIMEOUT);

        let mut buf = [0u8; 100];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((_bytes_count, _src_addr)) => match UdpMessage::from_bytes(buf.to_vec()) {
                    Ok(msg) => match msg {
                        UdpMessage::Ping { timestamp } => {
                            last_ping = Instant::now();
                            match socket.send_to(
                                // safety: should always serialize
                                &UdpMessage::Pong { timestamp }
                                    .to_bytes()
                                    .expect("err serializing pong"),
                                _src_addr,
                            ) {
                                Ok(_bytes_sent) => {
                                    println!("pong sent: {}", timestamp);
                                }
                                Err(e) => {
                                    eprintln!("err sending pong, {}", e);
                                }
                            }
                        }
                        _ => {
                            println!("unexpected message")
                        }
                    },
                    Err(e) => {
                        eprintln!("err deserealizing message: {}", e)
                    }
                },
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut || e.raw_os_error() == Some(35) =>
                {
                    if last_ping.elapsed() > timeout {
                        println!("client appears dead, no ping for {:?}", last_ping.elapsed());
                        shutdown.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("err recieving ping, {}", e);
                }
            }
        }
        Ok(())
    }
}
