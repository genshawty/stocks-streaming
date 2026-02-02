use std::io;
use thiserror::Error;

/// Errors that can occur during quote generation.
#[derive(Debug, Error)]
pub enum GeneratorError {
    /// An I/O error occurred while reading the file.
    ///
    /// Wraps the underlying [`std::io::Error`].
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Requested ticker symbol is not in the configured list.
    #[error("Ticker does not exist: {0}")]
    TickerNotExists(String),
}

/// Errors that can occur during quote processing.
#[derive(Debug, Error)]
pub enum ProcessorError {
    /// An I/O error occurred while reading the file.
    ///
    /// Wraps the underlying [`std::io::Error`].
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Failed to parse a command from the client.
    #[error("Parsing error: {0}")]
    ParseErr(#[from] ParseCommandErr),

    /// Quote generator encountered an error.
    #[error("Generator error: {0}")]
    GenErr(#[from] GeneratorError),

    /// Received a message type that the server doesn't handle.
    #[error("Unexpected command")]
    UnexpectedCommand,
}

/// Errors that can occur when parsing subscription commands.
#[derive(Debug, Error)]
pub enum ParseCommandErr {
    /// Command or protocol variant not recognized
    #[error("VariantNotFound: {0}")]
    VariantNotFound(#[from] strum::ParseError),

    /// The command string has fewer than the required fields
    #[error("Not enough arguments")]
    NotEnoughArguments,

    /// The IP address format is invalid
    #[error("Invalid IP address: {0}")]
    InvalidIpAddress(#[from] std::net::AddrParseError),

    /// The port number is invalid or out of range
    #[error("Invalid port number")]
    InvalidPort,

    /// Message data is not valid UTF-8.
    #[error("Invalid encoding")]
    Utf8Error(#[from] std::str::Utf8Error),

    /// Message size exceeds the 10MB limit (DoS protection).
    #[error("Message too big")]
    MsgToBig,

    /// Command type is not recognized.
    #[error("Unknown command")]
    UnknownCommand,
}
