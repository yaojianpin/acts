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

    #[error("{0}")]
    Store(String),

    #[error("{0}")]
    Action(String),

    #[error("{0}")]
    IoError(String),

    #[error("{0}")]
    Package(String),
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
        std::io::Error::new(ErrorKind::Other, val.to_string())
    }
}

impl From<rquickjs::Error> for ActError {
    fn from(error: rquickjs::Error) -> Self {
        ActError::Script(error.to_string())
    }
}

impl From<ActError> for rquickjs::Error {
    fn from(val: ActError) -> Self {
        std::io::Error::other(val.to_string()).into()
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

impl<'a> From<rquickjs::CaughtError<'a>> for ActError {
    fn from(error: rquickjs::CaughtError<'a>) -> Self {
        ActError::Script(error.to_string())
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
}
