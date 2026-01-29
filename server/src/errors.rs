use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeneratorError {
    /// An I/O error occurred while reading the file.
    ///
    /// Wraps the underlying [`std::io::Error`].
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}
