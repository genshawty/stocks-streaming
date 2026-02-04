use log::{debug, error, info, warn};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::errors::{ParseCommandErr, ProcessorError};
use crate::generator::QuoteGenerator;
use crate::types::{StockQuote, SubscribeCommand, TcpMessage, UdpMessage};
use crate::utils::read_tcp_message;

/// Maximum time (in seconds) without receiving a ping before considering client dead.
const MAX_PING_TIMEOUT: u64 = 5;

/// Handle to a subscriber's streaming thread.
/// Contains a shutdown signal to stop the subscriber's streaming and monitoring threads.
struct SubscriberHandle {
    /// Atomic flag used to signal the streaming thread to shut down.
    shutdown: Arc<AtomicBool>,
}

/// Information passed to a streaming thread when it starts.
struct SubscriberInfo {
    /// Unique identifier for this subscriber.
    id: u64,
    /// Channel for receiving stock quotes from the generator.
    channel: Receiver<StockQuote>,
    /// UDP address where quotes should be sent.
    udp_addr: SocketAddr,
    /// Atomic flag to signal shutdown.
    shutdown: Arc<AtomicBool>,
}

/// Main processor that manages TCP connections and subscriber streaming.
///
/// The Processor handles:
/// - Accepting TCP connections from clients
/// - Managing subscriber lifecycles (add/remove)
/// - Coordinating quote distribution through UDP
/// - Monitoring client health via ping/pong
pub(crate) struct Processor {
    /// TCP port for accepting client connections.
    port: u16,
    /// Quote generator shared across all subscribers.
    generator: Arc<Mutex<QuoteGenerator>>,
    /// Map of active subscribers by their unique ID.
    subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
    /// Atomic counter for generating unique subscriber IDs.
    next_id: Arc<AtomicU64>,
    /// Global shutdown flag for the TCP server.
    shutdown: Arc<AtomicBool>,
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
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Starts the TCP server and begins accepting client connections.
    ///
    /// This method:
    /// 1. Binds to the configured port
    /// 2. Spawns the quote generator thread
    /// 3. Accepts incoming connections in a loop
    /// 4. Spawns a handler thread for each connection
    ///
    /// The server runs until the shutdown flag is set.
    pub(crate) fn start_tcp_server(&self) -> Result<(), ProcessorError> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port))?;
        info!("TCP server started on port {}", self.port);
        listener.set_nonblocking(true)?;

        // Clone Arc parameters without holding the mutex lock
        let (prices, receivers, tickers_to_receivers, gen_shutdown) = {
            let generator_guard = self.generator.lock();
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
            if self.shutdown.load(Ordering::Relaxed) {
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

    /// Handles a single TCP client connection.
    ///
    /// Reads a command from the client, processes it, and sends back either
    /// an ACK (on success) or ERR (on failure) response.
    fn handle_request(
        mut stream: TcpStream,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
        next_id: Arc<AtomicU64>,
    ) -> Result<(), ProcessorError> {
        let msg = read_tcp_message(&mut stream)?;

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

    /// Sends a length-prefixed TCP message to the stream.
    ///
    /// Protocol: [4-byte length (big-endian)][message data (UTF-8 string)]
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
    ///
    /// Steps:
    /// 1. Generate unique subscriber ID
    /// 2. Create shutdown signal and quote channel
    /// 3. Register with quote generator
    /// 4. Insert into subscribers map
    /// 5. Spawn streaming thread
    ///
    /// Returns the subscriber ID on success.
    fn add_subscriber(
        cmd: SubscribeCommand,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
        next_id: Arc<AtomicU64>,
    ) -> Result<u64, ProcessorError> {
        // Generate unique ID
        let id = next_id.fetch_add(1, Ordering::Relaxed);

        // Create shutdown signal (shared between handle and thread)
        let shutdown = Arc::new(AtomicBool::new(false));

        // Create channel for receiving quotes from generator
        let (tx, rx) = mpsc::channel();

        {
            let mut generator_lock = generator.lock();
            generator_lock.add_receiver(id, tx, cmd.tickers_list.clone())?;
        }

        // Create subscriber info for streaming thread
        let sub_info = SubscriberInfo {
            id,
            channel: rx,
            udp_addr: SocketAddr::new(cmd.ip.into(), cmd.port),
            shutdown: Arc::clone(&shutdown),
        };
        let subscribers_clone = subscribers.clone();

        subscribers.write().insert(
            id,
            SubscriberHandle {
                shutdown: Arc::clone(&shutdown),
            },
        );

        // Spawn streaming thread (JoinHandle is intentionally dropped as cleanup happens via shutdown signal)
        let _ = thread::spawn(move || {
            if let Err(e) = Self::start_streaming(sub_info, generator, subscribers_clone) {
                error!("Streaming thread error: {}", e);
            }
        });

        info!(
            "Added subscriber {} for tickers: {:?}",
            id, cmd.tickers_list
        );
        Ok(id)
    }

    /// Removes a subscriber and cleans up resources.
    ///
    /// Steps:
    /// 1. Remove subscriber handle from map
    /// 2. Signal shutdown to streaming thread
    /// 3. Remove receiver from quote generator
    ///
    /// This is called when a subscriber's connection ends or times out.
    fn remove_subscriber(
        id: u64,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
    ) {
        // 1. Remove from subscribers map
        let handle = subscribers.write().remove(&id);

        if let Some(handle) = handle {
            info!("Removing subscriber {}", id);

            // 2. Signal shutdown
            handle.shutdown.store(true, Ordering::Relaxed);

            // 3. Remove from generator
            let (recv, tickers_to_recv) = {
                let generator_lock = generator.lock();
                (
                    generator_lock.receivers.clone(),
                    generator_lock.tickers_to_receivers.clone(),
                )
            };

            QuoteGenerator::remove_receiver(id, &recv, &tickers_to_recv);

            info!("Subscriber {} removed", id);
        }
    }

    /// Streaming loop that receives quotes and sends via UDP.
    ///
    /// Creates a UDP socket and spawns a monitoring thread to handle ping/pong.
    /// Continuously receives quotes from the generator and sends them to the client.
    /// Exits when shutdown signal is received or channel is disconnected.
    fn start_streaming(
        info: SubscriberInfo,
        generator: Arc<Mutex<QuoteGenerator>>,
        subscribers: Arc<RwLock<HashMap<u64, SubscriberHandle>>>,
    ) -> Result<(), ProcessorError> {
        // Create UDP socket for this subscriber
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to create UDP socket for subscriber {}: {}",
                    info.id, e
                );
                Processor::remove_subscriber(info.id, generator, subscribers);
                return Ok(());
            }
        };

        info!(
            "Started streaming for subscriber {} to {}",
            info.id, info.udp_addr
        );
        let socket_clone = socket.try_clone().expect("err cloning socket");
        let shutdown_clone = info.shutdown.clone();
        let id = info.id;

        let monitor_handle =
            thread::spawn(move || Processor::monitor_connection(id, socket_clone, shutdown_clone));

        loop {
            // Check shutdown signal
            if info.shutdown.load(Ordering::Relaxed) {
                info!("Subscriber {} received shutdown signal", info.id);

                break;
            }

            // Receive quote with timeout (non-blocking)
            match info.channel.recv_timeout(Duration::from_millis(100)) {
                Ok(quote) => {
                    let msg = UdpMessage::Quote(quote);
                    // Serialize quote to bytes
                    match msg.to_bytes() {
                        Ok(bytes) => {
                            debug!("Sending quote to subscriber {}", info.id);
                            // Send as single UDP datagram
                            if let Err(e) = socket.send_to(&bytes, info.udp_addr) {
                                error!("UDP send failed for subscriber {}: {}", info.id, e);
                            }
                        }
                        Err(e) => {
                            error!(
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
                    info!("Channel disconnected for subscriber {}", info.id);
                    break;
                }
            }
        }
        info.shutdown.store(true, Ordering::Relaxed);
        if let Ok(res) = monitor_handle.join() {
            res?
        };

        Processor::remove_subscriber(info.id, generator, subscribers);
        info!("Streaming ended for subscriber {}", info.id);
        Ok(())
    }

    /// Monitors client health via UDP ping/pong messages.
    ///
    /// Listens for Ping messages from the client and responds with Pong.
    /// If no ping is received within MAX_PING_TIMEOUT seconds, signals shutdown.
    /// This runs in a separate thread spawned by start_streaming.
    fn monitor_connection(
        id: u64,
        socket: UdpSocket,
        shutdown: Arc<AtomicBool>,
    ) -> Result<(), ProcessorError> {
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("err setting read timeout");
        // Monitor last pings and consider client dead after MAX_PING_TIMEOUT seconds
        let mut last_ping = None;
        let start_time = Instant::now();
        let timeout = Duration::from_secs(MAX_PING_TIMEOUT);

        let mut buf = [0u8; 1000];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((_bytes_count, _src_addr)) => {
                    match UdpMessage::from_bytes(buf[.._bytes_count].to_vec()) {
                        Ok(msg) => match msg {
                            UdpMessage::Ping { timestamp } => {
                                last_ping = Some(Instant::now());
                                match socket.send_to(
                                    // safety: should always serialize
                                    &UdpMessage::Pong { timestamp }
                                        .to_bytes()
                                        .expect("err serializing pong"),
                                    _src_addr,
                                ) {
                                    Ok(_bytes_sent) => {
                                        debug!("Pong sent: {}", timestamp);
                                    }
                                    Err(e) => {
                                        error!("Error sending pong: {}", e);
                                    }
                                }
                            }
                            _ => {
                                warn!("Unexpected message received (expected Ping)")
                            }
                        },
                        Err(e) => {
                            error!("Error deserializing message: {}", e)
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock
                        || e.raw_os_error() == Some(35) =>
                {
                    match last_ping {
                        Some(lp) if lp.elapsed() > timeout => {
                            info!(
                                "[{}] Client appears dead, no ping for {:?}",
                                id,
                                lp.elapsed()
                            );
                            shutdown.store(true, Ordering::Relaxed);
                            break;
                        }
                        None if start_time.elapsed() > 3 * timeout => {
                            info!(
                                "[{}] Client appears dead, no ping for {:?}",
                                id,
                                start_time.elapsed()
                            );
                            shutdown.store(true, Ordering::Relaxed);
                            break;
                        }
                        _ => continue,
                    }
                }
                Err(e) => {
                    error!("Error receiving ping: {}", e);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// Helper to create a connected pair of TCP streams for testing.
    fn create_tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let client = thread::spawn(move || TcpStream::connect(addr).unwrap());
        let (server, _) = listener.accept().unwrap();
        let client = client.join().unwrap();

        (client, server)
    }

    #[test]
    fn test_send_and_read_tcp_message_ack() {
        let (mut client, mut server) = create_tcp_pair();

        // Send ACK from server to client
        let ack_msg = TcpMessage::Ack;
        Processor::send_tcp_message(&mut server, &ack_msg).unwrap();

        // Read ACK on client side
        let received = read_tcp_message(&mut client).unwrap();
        assert!(matches!(received, TcpMessage::Ack));
    }

    #[test]
    fn test_send_and_read_tcp_message_err() {
        let (mut client, mut server) = create_tcp_pair();

        // Send ERR from server to client
        let err_msg = TcpMessage::Err("Test error message".to_string());
        Processor::send_tcp_message(&mut server, &err_msg).unwrap();

        // Read ERR on client side
        let received = read_tcp_message(&mut client).unwrap();
        match received {
            TcpMessage::Err(msg) => assert_eq!(msg, "Test error message"),
            _ => panic!("Expected Err variant"),
        }
    }

    #[test]
    fn test_read_tcp_message_with_size_limit() {
        let (mut client, mut server) = create_tcp_pair();

        // Try to send a message that's too large (> 10MB)
        let huge_size: u32 = 11_000_000; // 11MB
        server.write_all(&huge_size.to_be_bytes()).unwrap();
        server.flush().unwrap();

        // Reading should fail with MsgTooBig error
        let result = read_tcp_message(&mut client);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_tcp_message_empty_disconnect() {
        let (mut client, _server) = create_tcp_pair();
        // Drop server to close connection
        drop(_server);

        // Reading should fail with connection closed error
        let result = read_tcp_message(&mut client);
        assert!(result.is_err());
    }

    #[test]
    fn test_tcp_message_round_trip_cmd() {
        use crate::types::{Commands, Protocol, SubscribeCommand};
        use std::net::Ipv4Addr;

        let (mut client, mut server) = create_tcp_pair();

        // Create a CMD message
        let cmd = SubscribeCommand {
            cmd: Commands::Stream,
            protocol: Protocol::Udp,
            ip: Ipv4Addr::new(127, 0, 0, 1),
            port: 8080,
            tickers_list: vec!["AAPL".to_string(), "GOOGL".to_string()],
        };
        let cmd_msg = TcpMessage::Cmd(cmd);

        // Send from client to server
        Processor::send_tcp_message(&mut client, &cmd_msg).unwrap();

        // Read on server side
        let received = read_tcp_message(&mut server).unwrap();
        match received {
            TcpMessage::Cmd(received_cmd) => {
                assert_eq!(received_cmd.cmd, Commands::Stream);
                assert_eq!(received_cmd.protocol, Protocol::Udp);
                assert_eq!(received_cmd.ip, Ipv4Addr::new(127, 0, 0, 1));
                assert_eq!(received_cmd.port, 8080);
                assert_eq!(received_cmd.tickers_list, vec!["AAPL", "GOOGL"]);
            }
            _ => panic!("Expected Cmd variant"),
        }
    }

    #[test]
    fn test_message_length_validation() {
        let (mut client, mut server) = create_tcp_pair();

        // Send valid small message
        let small_msg = TcpMessage::Ack;
        Processor::send_tcp_message(&mut server, &small_msg).unwrap();
        let received = read_tcp_message(&mut client).unwrap();
        assert!(matches!(received, TcpMessage::Ack));

        // Send message near the limit (should succeed)
        let large_err = "x".repeat(1_000_000); // 1MB
        let large_msg = TcpMessage::Err(large_err.clone());
        Processor::send_tcp_message(&mut server, &large_msg).unwrap();
        let received = read_tcp_message(&mut client).unwrap();
        match received {
            TcpMessage::Err(msg) => assert_eq!(msg.len(), 1_000_000),
            _ => panic!("Expected Err variant"),
        }
    }
}
