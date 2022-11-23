use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Deserialize, Serialize, Error, Debug, Clone, PartialEq)]
pub enum ActError {
    #[error("Error happend when parse yml ({0})")]
    ParseError(String),

    #[error("Error happend when parse message \"{0}\", the format should be workflowid-jobid or workflowid-jobid-stepid")]
    MessageIdError(String),

    #[error("Invalid config file format")]
    ConfigError,

    #[error("Scher error: {0}")]
    ScherError(String),

    #[error("Invalid if condition in branch")]
    BranchIfError,

    #[error("Invalid output in job ({0})")]
    OutputError(String),

    #[error("Error happened by step ({0})")]
    StepError(String),

    #[error("Error happened by branch ({0})")]
    BranchError(String),

    #[error("Cannot find next flow ({0})")]
    NextError(String),

    #[error("Needs error ({0})")]
    NeedsError(String),

    #[error("script error ({0})")]
    ScriptError(String),

    #[error("Cannot find current user")]
    CurrentUserNotFoundError,

    #[error("Step sub error with ({0})")]
    SubjectError(String),

    #[error("Cannot find message ({0})")]
    MessageNotFoundError(String),

    #[error("env error for ({0})")]
    EnvError(String),

    #[error("store error with ({0})")]
    StoreError(String),
}

impl Into<String> for ActError {
    fn into(self) -> String {
        self.to_string()
    }
}
