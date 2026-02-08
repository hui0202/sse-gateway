//! Error types for SSE Gateway

use thiserror::Error;

/// Result type alias using the library's Error type
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in SSE Gateway
#[derive(Error, Debug)]
pub enum Error {
    /// Source-related errors
    #[error("Message source error: {0}")]
    Source(#[from] anyhow::Error),

    /// Storage-related errors
    #[error("Storage error: {0}")]
    Storage(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// Server errors
    #[error("Server error: {0}")]
    Server(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
