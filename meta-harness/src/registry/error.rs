use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),

    #[error("Usage tree not found: {0}")]
    UsageTreeNotFound(String),

    #[error("Component already exists: {0}")]
    ComponentAlreadyExists(String),

    #[error("Invalid lifecycle transition: {0}")]
    InvalidTransition(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
