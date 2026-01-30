use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::ops::Sub;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::errors::ProcessorError;
use crate::types::SubscribeCommand;

const HEADER_LEN: usize = 4;

pub(crate) struct Processor {
    port: u16,

    shutdown: Arc<RwLock<bool>>,
}

impl Processor {
    pub(crate) fn start() {}
    fn start_tcp_server(&self) -> Result<(), ProcessorError> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port))?;
        println!("TCP server started on port {}", self.port);
        listener.set_nonblocking(true)?;

        loop {
            if *self.shutdown.read().unwrap() {
                println!("TCP server shutting down");
                break;
            }

            match listener.accept() {
                Ok((stream, addr)) => {
                    println!("New worker connected: {}", addr);

                    thread::spawn(move || {
                        if let Err(e) = Self::handle_request(stream) {
                            eprintln!("Worker handler error: {}", e);
                        }
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => eprintln!("Accept error: {}", e),
            }
        }

        Ok(())
    }

    fn handle_request(mut stream: TcpStream) -> Result<(), ProcessorError> {
        loop {
            let mut len_buf = [0u8; HEADER_LEN];
            match stream.read_exact(&mut len_buf) {
                Ok(()) => {
                    let msg_len = u32::from_be_bytes(len_buf) as usize;
                    let mut data = vec![0u8; msg_len];
                    match stream.read_exact(&mut data) {
                        Ok(()) => {
                            let cmd = SubscribeCommand::from_str(
                                std::str::from_utf8(&data)
                                    .map_err(|e| ProcessorError::ParseErr(e.into()))?,
                            )?;
                            // todo
                        }
                        Err(e) => {
                            // todo
                            eprint!("error reading data: {}", e)
                        }
                    }
                }
                Err(e) => {
                    // todo
                    eprint!("error reading header: {}", e)
                }
            }
        }
        Ok(())
    }
}
