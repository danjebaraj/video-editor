use thiserror::Error;

/// Core error. `code` values are stable strings the shells can switch on.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    InvalidProject(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Environment(String),
    #[error("{0}")]
    Denied(String),
    #[error("{0}")]
    Budget(String),
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::Io(_) => "io",
            Error::Json(_) => "json",
            Error::InvalidProject(_) => "invalid_project",
            Error::NotFound(_) => "not_found",
            Error::Environment(_) => "environment",
            Error::Denied(_) => "denied",
            Error::Budget(_) => "budget",
            Error::Http(_) => "http",
            Error::Other(_) => "other",
        }
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
