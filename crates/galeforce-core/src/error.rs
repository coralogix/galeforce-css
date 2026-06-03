use thiserror::Error;

use crate::SourceSpan;

#[derive(Debug, Error)]
pub enum GaleforceError {
    #[error("invalid candidate `{candidate}`: {message}")]
    InvalidCandidate {
        candidate: String,
        message: String,
        span: Option<SourceSpan>,
    },

    #[error("unsupported feature `{feature}`: {message}")]
    Unsupported { feature: String, message: String },

    #[error("config error: {0}")]
    Config(String),

    #[error("CSS parse error: {0}")]
    CssParse(String),

    #[error("theme lookup failed: {path}")]
    Theme { path: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
