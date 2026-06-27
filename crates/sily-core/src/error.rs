//! Error types for the sily core. Core ops are pure; errors describe malformed
//! or inconsistent session graphs, plus pass-through I/O/JSON from adapters.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error("cycle detected in parent chain at {0}")]
    Cycle(String),

    #[error("session has no root message (every message has a parent)")]
    NoRoot,

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("not supported by {0}")]
    Unsupported(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
