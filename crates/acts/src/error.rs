use core::fmt;
use serde::{Deserialize, Serialize};
use std::{io::ErrorKind, string::FromUtf8Error};
use thiserror::Error;

use crate::{Result, Vars};

#[derive(Deserialize, Serialize, Error, Debug, Clone, PartialEq)]
pub enum ActError {
    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    Convert(String),

    #[error("{0}")]
    Script(String),

    #[error("ecode: {ecode}, message: {message}")]
    Exception { ecode: String, message: String },

    #[error("{0}")]
    Model(String),

    #[error("{0}")]
    Runtime(String),

    /// The bounded scheduler queue is at capacity. Fresh work must be retried;
    /// durable `next` work remains in the outbox and is replayed later.
    #[error("scheduler queue is full")]
    QueueFull,

    /// The engine is closing: the scheduler queue no longer accepts work (or
    /// the store writer has been drained). Not a business failure — a job
    /// interrupted by it must leave the durable state replayable, because the
    /// next engine start re-drives exactly what this refusal interrupted.
    #[error("scheduler is shutting down")]
    Shutdown,

    #[error("{0}")]
    Store(String),

    #[error("{0}")]
    Action(String),

    #[error("{0}")]
    IoError(String),

    #[error("{0}")]
    Package(String),

    /// The request carried no token, or a token that matches no role, while
    /// an ACL is configured. Transports map this to "unauthenticated".
    #[error("{0}")]
    Unauthenticated(String),

    /// The caller is authenticated but not allowed to run the operation.
    #[error("{0}")]
    Denied(String),

    /// Another live instance holds the database's exclusive lease. Startup
    /// refuses to run an engine on that database — a second engine over one
    /// database duplicates recovery and scheduling, which is what the lease
    /// exists to prevent. The holder is named in the message; a crashed
    /// holder's lease expires on its own and a restart may claim it then.
    #[error("database lease is held by another instance: {0}")]
    LeaseHeld(String),

    /// This instance's exclusive database lease was taken over (its renewal
    /// was refused). Every write is refused from then on, so the instance
    /// cannot overwrite the new holder's rows: the engine's work has to move
    /// to the holder.
    #[error("database lease lost: this instance no longer owns the database")]
    LeaseLost,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct Error {
    #[serde(default)]
    pub ecode: String,
    #[serde(default)]
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = serde_json::to_string(self).unwrap();
        f.write_str(&text)
    }
}

impl Error {
    pub fn new(message: &str, ecode: &str) -> Self {
        Self {
            message: message.to_string(),
            ecode: ecode.to_string(),
        }
    }

    pub fn from_var(value: &Vars) -> Result<Self> {
        serde_json::from_value::<Self>(value.clone().into()).map_err(|err| err.into())
    }
}

impl ActError {
    /// Whether this error reports the engine closing rather than a failure of
    /// the work itself: the scheduler queue refused the dispatch (or the store
    /// writer is already drained). A job interrupted by it must not persist a
    /// business failure (`Error` state, closed outbox record) — the durable
    /// state has to stay exactly as the next engine start expects it, so the
    /// boot resume can re-drive what this shutdown cut short.
    pub fn is_shutdown(&self) -> bool {
        matches!(self, ActError::Shutdown)
    }
}

impl From<ActError> for String {
    fn from(val: ActError) -> Self {
        val.to_string()
    }
}

impl From<ActError> for Vars {
    fn from(val: ActError) -> Self {
        match val {
            ActError::Exception { ecode, message } => {
                Vars::new().with("ecode", ecode).with("message", message)
            }
            err => Vars::new()
                .with("ecode", "")
                .with("message", err.to_string()),
        }
    }
}

impl From<ActError> for Error {
    fn from(val: ActError) -> Self {
        match val {
            ActError::Exception { ecode, message } => Error { ecode, message },
            err => Error {
                ecode: "".to_string(),
                message: err.to_string(),
            },
        }
    }
}

impl From<std::io::Error> for ActError {
    fn from(error: std::io::Error) -> Self {
        ActError::IoError(error.to_string())
    }
}

impl From<ActError> for std::io::Error {
    fn from(val: ActError) -> Self {
        #[allow(clippy::io_other_error)]
        std::io::Error::new(ErrorKind::Other, val.to_string())
    }
}

impl From<FromUtf8Error> for ActError {
    fn from(_: FromUtf8Error) -> Self {
        ActError::Runtime("Error with utf-8 string convert".to_string())
    }
}

impl From<serde_json::Error> for ActError {
    fn from(error: serde_json::Error) -> Self {
        ActError::Convert(error.to_string())
    }
}

impl From<jsonschema::ValidationError<'_>> for ActError {
    fn from(error: jsonschema::ValidationError<'_>) -> Self {
        ActError::Runtime(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{ActError, Error, Vars};

    #[test]
    fn engine_error_default() {
        let err = Error::default();
        assert_eq!(err.message, "");
        assert_eq!(err.ecode, "");
    }

    #[test]
    fn engine_error_json_full() {
        let err = Error::new("abc", "err1");
        let v = serde_json::to_value(err).unwrap();
        assert_eq!(v, json!({ "ecode": "err1", "message": "abc" }))
    }

    #[test]
    fn engine_error_from_value() {
        let err = Vars::new().with("ecode", "err1").with("message", "test");
        let v = Error::from_var(&err).unwrap();
        assert_eq!(v.ecode, "err1");
        assert_eq!(v.message, "test");
    }

    #[test]
    fn engine_act_error_into() {
        let err = ActError::Action("error message".to_string());
        let v: Error = err.into();
        assert_eq!(v.message, "error message");
        assert_eq!(v.ecode, "");
    }

    #[test]
    fn engine_act_exception_into() {
        let err = ActError::Exception {
            ecode: "err1".to_string(),
            message: "error message".to_string(),
        };
        let v: Error = err.into();
        assert_eq!(v.message, "error message");
        assert_eq!(v.ecode, "err1");
    }

    #[test]
    fn error_display_is_json() {
        let err = Error::new("boom", "e1");
        assert_eq!(
            err.to_string(),
            json!({"ecode": "e1", "message": "boom"}).to_string()
        );
    }

    #[test]
    fn act_error_variants_display_and_is_shutdown() {
        assert_eq!(ActError::Config("cfg".to_string()).to_string(), "cfg");
        assert_eq!(ActError::QueueFull.to_string(), "scheduler queue is full");
        assert_eq!(ActError::Shutdown.to_string(), "scheduler is shutting down");
        assert_eq!(
            ActError::LeaseHeld("host".to_string()).to_string(),
            "database lease is held by another instance: host"
        );
        assert_eq!(
            ActError::LeaseLost.to_string(),
            "database lease lost: this instance no longer owns the database"
        );

        assert!(ActError::Shutdown.is_shutdown());
        assert!(!ActError::QueueFull.is_shutdown());
    }

    #[test]
    fn act_error_into_string_and_vars() {
        let text: String = ActError::Action("action failed".to_string()).into();
        assert_eq!(text, "action failed");

        let vars: Vars = ActError::Exception {
            ecode: "e1".to_string(),
            message: "boom".to_string(),
        }
        .into();
        assert_eq!(vars.get::<String>("ecode").unwrap(), "e1");
        assert_eq!(vars.get::<String>("message").unwrap(), "boom");

        let vars: Vars = ActError::Action("plain".to_string()).into();
        assert_eq!(vars.get::<String>("ecode").unwrap(), "");
        assert_eq!(vars.get::<String>("message").unwrap(), "plain");
    }

    #[test]
    fn act_error_io_and_utf8_and_json_conversions() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: ActError = io.into();
        assert!(matches!(err, ActError::IoError(_)));
        assert!(err.to_string().contains("missing"));

        let back: std::io::Error = err.into();
        assert!(back.to_string().contains("missing"));

        let utf8 = String::from_utf8(vec![0xff]).unwrap_err();
        let err: ActError = utf8.into();
        assert!(matches!(err, ActError::Runtime(_)));
        assert_eq!(err.to_string(), "Error with utf-8 string convert");

        let json_err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let err: ActError = json_err.into();
        assert!(matches!(err, ActError::Convert(_)));
    }

    #[test]
    fn jsonschema_validation_error_converts() {
        let schema = json!({ "type": "string" });
        let value = json!(1);
        let validator = jsonschema::validator_for(&schema).unwrap();
        let err = validator.validate(&value).unwrap_err();
        let err: ActError = err.into();
        assert!(matches!(err, ActError::Runtime(_)));
        assert!(!err.to_string().is_empty());
    }
}
