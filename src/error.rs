use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use thiserror::Error;

#[derive(Deserialize, Serialize, Error, Debug, Clone, PartialEq)]
pub enum ActError {
    #[error("{0}")]
    Convert(String),

    #[error("{0}")]
    Script(String),

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
