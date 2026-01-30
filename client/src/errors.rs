use std::io;
use thiserror::Error;

/// Errors that can occur during quote processing.
#[derive(Debug, Error)]
pub enum WorkerError {
    /// An I/O error occurred while reading the file.
    ///
    /// Wraps the underlying [`std::io::Error`].
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}
