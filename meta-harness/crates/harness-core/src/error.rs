use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("{operation} is not supported by this harness")]
    Unsupported { operation: &'static str },

    #[error("invalid configuration: {reason}")]
    InvalidConfig { reason: String },

    #[error("not found: {what}")]
    NotFound { what: String },

    #[error("authentication required: {reason}")]
    AuthRequired { reason: String },

    #[error("harness failed: {reason}")]
    Provider { reason: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
