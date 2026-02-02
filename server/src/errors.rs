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

    #[error("Parsing error: {0}")]
    ParseErr(#[from] ParseCommandErr),

    #[error("Generator error: {0}")]
    GenErr(#[from] GeneratorError),

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

    /// add documentation
    #[error("Invalid encoding")]
    Utf8Error(#[from] std::str::Utf8Error),

    #[error("Unknown command")]
    UnknownCommand,
}
