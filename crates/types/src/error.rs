//! Error type shared across layers, with a classification used for retry decisions.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// Network blips, 5xx, timeouts. Safe to retry.
    Transient,
    /// Rate limited (429). Retry after the given delay.
    RateLimited,
    /// Credentials or permissions. Not retryable.
    Auth,
    /// The request/record is invalid for the destination. Not retryable; record-level failure.
    Validation,
    /// Configuration or spec error. Not retryable.
    Config,
    /// Blocked by policy (network policy, approval, sandbox). Never retried.
    Policy,
    /// Requested capability is not supported by the system.
    Unsupported,
    NotFound,
    Internal,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{class:?}: {message}")]
pub struct MaError {
    pub class: ErrorClass,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl MaError {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        MaError { class, message: message.into(), retry_after: None }
    }
    pub fn transient(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Transient, m)
    }
    pub fn config(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Config, m)
    }
    pub fn policy(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Policy, m)
    }
    pub fn validation(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Validation, m)
    }
    pub fn unsupported(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Unsupported, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::NotFound, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Internal, m)
    }
    pub fn auth(m: impl Into<String>) -> Self {
        Self::new(ErrorClass::Auth, m)
    }
    pub fn rate_limited(m: impl Into<String>, retry_after: Option<Duration>) -> Self {
        MaError { class: ErrorClass::RateLimited, message: m.into(), retry_after }
    }
    pub fn is_retryable(&self) -> bool {
        matches!(self.class, ErrorClass::Transient | ErrorClass::RateLimited)
    }
}

impl From<std::io::Error> for MaError {
    fn from(e: std::io::Error) -> Self {
        MaError::new(ErrorClass::Internal, format!("I/O error: {e}"))
    }
}

impl From<serde_json::Error> for MaError {
    fn from(e: serde_json::Error) -> Self {
        MaError::new(ErrorClass::Validation, format!("JSON error: {e}"))
    }
}

pub type MaResult<T> = Result<T, MaError>;
