use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RespData<T> {
    pub code: RespStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RespStatus {
    Ok = 200,
    Error = 500,
}

impl<T> RespData<T> {
    pub fn ok(data: T) -> Self
    where
        T: Serialize,
    {
        RespData {
            code: RespStatus::Ok,
            data: Some(data),
            message: None,
            details: None,
        }
    }

    pub fn err(message: &str) -> Self {
        RespData {
            code: RespStatus::Error,
            data: None,
            message: Some(message.to_string()),
            details: None,
        }
    }

    pub fn err_with_details(message: &str, details: &str) -> Self {
        RespData {
            code: RespStatus::Error,
            data: None,
            message: Some(message.to_string()),
            details: Some(details.to_string()),
        }
    }
}

impl<T> IntoResponse for RespData<T>
where
    T: Serialize,
{
    fn into_response(self) -> axum::response::Response {
        let status = if self.code == RespStatus::Error {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        };
        (status, Json(serde_json::to_value(self).unwrap())).into_response()
    }
}

#[derive(Debug, Serialize)]
pub struct AppError {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self.details {
            Some(details) => {
                RespData::<()>::err_with_details(&self.message, &details).into_response()
            }
            None => RespData::<()>::err(&self.message).into_response(),
        }
    }
}

impl From<&str> for AppError {
    fn from(value: &str) -> Self {
        Self {
            message: value.to_string(),
            details: None,
        }
    }
}

impl From<acts::ActError> for AppError {
    fn from(value: acts::ActError) -> Self {
        Self {
            message: value.to_string(),
            details: None,
        }
    }
}

impl From<validator::ValidationErrors> for AppError {
    fn from(value: validator::ValidationErrors) -> Self {
        Self {
            message: value.to_string(),
            details: None,
        }
    }
}
