use core::fmt;
use serde::{Deserialize, Serialize};
use std::{io::ErrorKind, string::FromUtf8Error};
use thiserror::Error;

#[derive(Deserialize, Serialize, Error, Debug, Clone, PartialEq)]
pub enum ActError {
    #[error("{0}")]
    Convert(String),

    #[error("{0}")]
    Script(String),

    #[error("{0}")]
    Exception(String),

    #[error("{0}")]
    Model(String),

    #[error("{0}")]
    Runtime(String),

    #[error("{0}")]
    Adapter(String),

    #[error("{0}")]
    Store(String),

    #[error("{0}")]
    Action(String),

    #[error("{0}")]
    IoError(String),
}

#[derive(Debug, Clone)]
pub struct Error {
    pub key: Option<String>,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(key) = &self.key {
            f.write_fmt(format_args!("{}:{}", &self.message, key))
        } else {
            f.write_fmt(format_args!("{}", &self.message))
        }
    }
}

impl Default for Error {
    fn default() -> Self {
        Self {
            key: None,
            message: Default::default(),
        }
    }
}

impl Error {
    pub fn parse(s: &str) -> Error {
        let parts = s.split(':').collect::<Vec<_>>();

        if parts.len() == 2 {
            return Error {
                message: parts[0].to_string(),
                key: Some(parts[1].to_string()),
            };
        }

        Error {
            key: None,
            message: s.to_string(),
        }
    }
}

impl Into<String> for ActError {
    fn into(self) -> String {
        self.to_string()
    }
}

impl From<std::io::Error> for ActError {
    fn from(error: std::io::Error) -> Self {
        ActError::IoError(error.to_string())
    }
}

impl Into<std::io::Error> for ActError {
    fn into(self) -> std::io::Error {
        std::io::Error::new(ErrorKind::Other, self.to_string())
    }
}

impl From<rquickjs::Error> for ActError {
    fn from(error: rquickjs::Error) -> Self {
        ActError::Script(error.to_string())
    }
}

impl Into<rquickjs::Error> for ActError {
    fn into(self) -> rquickjs::Error {
        std::io::Error::other(self.to_string()).into()
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
