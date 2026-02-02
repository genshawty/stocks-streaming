use std::io;
use thiserror::Error;

/// Errors that can occur in the client worker.
#[derive(Debug, Error)]
pub enum WorkerError {
    /// Network or socket I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}
