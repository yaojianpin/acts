use axum::{Json, http::StatusCode, response::IntoResponse};
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
    /// HTTP status the error is answered with. Action errors carry their own
    /// kind (401/403), refused client input (validation, out-of-range
    /// paging) a 400, everything else a 500.
    #[serde(skip)]
    status: StatusCode,
}

impl AppError {
    /// The request itself is malformed or exceeds a server bound — a client
    /// fault, not a server failure.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: None,
            status: StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = match self.details {
            Some(details) => RespData::<()>::err_with_details(&self.message, &details),
            None => RespData::<()>::err(&self.message),
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<&str> for AppError {
    fn from(value: &str) -> Self {
        Self {
            message: value.to_string(),
            details: None,
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<acts::ActError> for AppError {
    fn from(value: acts::ActError) -> Self {
        Self {
            message: value.to_string(),
            details: None,
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<acts::actions::Error> for AppError {
    fn from(value: acts::actions::Error) -> Self {
        let status = match value {
            acts::actions::Error::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            acts::actions::Error::Denied(_) => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            message: value.to_string(),
            details: None,
            status,
        }
    }
}

impl From<validator::ValidationErrors> for AppError {
    fn from(value: validator::ValidationErrors) -> Self {
        Self {
            message: value.to_string(),
            details: None,
            status: StatusCode::BAD_REQUEST,
        }
    }
}
