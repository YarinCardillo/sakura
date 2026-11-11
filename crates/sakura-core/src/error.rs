//! The one error type that crosses the bridge.

use crate::said::Said;
use serde::Serialize;
use std::fmt;

/// Machine-readable reason, for the window to branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    BoxMissing,
    DockerDown,
    InvalidArgument,
    BoxFailed,
    NotFound,
    /// `box` printed its usage: this version does not have the command.
    Unsupported,
    /// `box cancel` found no queued or running check to cancel.
    NothingToCancel,
    TerminalClosed,
    Internal,
}

/// A failure the window can show: `said` names the sentence for a person (the window finds its words),
/// `code` is for branching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BridgeError {
    pub code: ErrorCode,
    pub said: Said,
    /// What `box` or the system itself wrote, kept as data: shown only under the details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl BridgeError {
    /// Builds an error from a code and what to say of it.
    pub fn new(code: ErrorCode, said: impl Into<Said>) -> Self {
        Self {
            code,
            said: said.into(),
            details: None,
        }
    }

    /// The same error with the line `box` wrote kept as its details.
    pub fn with_details(mut self, details: Option<String>) -> Self {
        self.details = details;
        self
    }

    /// A caller-supplied value failed validation before any process was started.
    pub fn invalid(said: impl Into<Said>) -> Self {
        Self::new(ErrorCode::InvalidArgument, said)
    }

    /// Something went wrong inside the core itself.
    pub fn internal(said: impl Into<Said>) -> Self {
        Self::new(ErrorCode::Internal, said)
    }

    /// `box` ran and reported a failure.
    pub fn failed(said: impl Into<Said>) -> Self {
        Self::new(ErrorCode::BoxFailed, said)
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.said.key.0)?;
        match &self.details {
            Some(details) => write!(f, " ({details})"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Result alias used by every bridge call.
pub type BridgeResult<T> = Result<T, BridgeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_codes_in_kebab_case() {
        let error = BridgeError::new(ErrorCode::BoxMissing, Said::new("error.box.missing"));
        let json = serde_json::to_string(&error).unwrap();
        assert_eq!(
            json,
            r#"{"code":"box-missing","said":{"key":"error.box.missing"}}"#
        );
        let json = serde_json::to_string(&ErrorCode::InvalidArgument).unwrap();
        assert_eq!(json, r#""invalid-argument""#);
    }
}
