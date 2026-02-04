use crate::{errors::ParseCommandErr, types::TcpMessage};
use log::error;
use std::io::Read;
use std::net::TcpStream;

/// Reads a length-prefixed TCP message from the stream.
///
/// Protocol: [4-byte length (big-endian)][message data (UTF-8 string)]
///
/// Returns an error if:
/// - Connection is closed unexpectedly
/// - Message length exceeds 10MB (DoS protection)
/// - Message data is not valid UTF-8
/// - Message cannot be parsed as TcpMessage
pub fn read_tcp_message(stream: &mut TcpStream) -> Result<TcpMessage, ParseCommandErr> {
    // Read 4-byte message length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            error!("Client disconnected while reading message length");
        } else {
            error!("Error reading message length: {}", e);
        }
        ParseCommandErr::from(e)
    })?;

    // Read message data
    let msg_len = u32::from_be_bytes(len_buf) as usize;
    if msg_len > 1e7 as usize {
        return Err(ParseCommandErr::MsgToBig.into());
    }
    let mut data = vec![0u8; msg_len];
    stream.read_exact(&mut data).map_err(|e| {
        error!("Error reading message data (length: {}): {}", msg_len, e);
        ParseCommandErr::from(e)
    })?;

    // Parse UTF-8 string
    let msg_str = std::str::from_utf8(&data).map_err(|e| {
        error!("Invalid UTF-8 in message data: {}", e);
        ParseCommandErr::from(e)
    })?;

    // Parse TcpMessage
    TcpMessage::from_string(msg_str).map_err(|e| {
        error!("Failed to parse TCP message '{}': {}", msg_str, e);
        e.into()
    })
}
