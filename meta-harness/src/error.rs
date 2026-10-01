use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Process exited unexpectedly with code: {0}")]
    ProcessExit(i32),

    #[error("Process was terminated by signal")]
    ProcessTerminated,

    #[error("Session is not active or has disconnected")]
    SessionClosed,

    #[error("Unknown or invalid approval request ID: {0}")]
    InvalidRequestId(String),

    #[error("Invalid decision for request {request_id}: {reason}")]
    InvalidDecision {
        request_id: String,
        reason: String,
    },

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Channel communication error: {0}")]
    ChannelError(String),

    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Provider setup error ({operation}): {detail}")]
    ProviderSetup {
        operation: String,
        detail: String,
    },

    #[error("Provider driver error: {detail}")]
    ProviderDriver {
        detail: String,
    },
}
