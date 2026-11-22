use acts::export::error::new_correlation_id;
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

    /// The error envelope's shape, for the contract tests: the live error
    /// path answers through [`AppError`] (the stable-code JSON), never
    /// through this constructor.
    #[cfg(test)]
    pub fn err(message: &str) -> Self {
        RespData {
            code: RespStatus::Error,
            data: None,
            message: Some(message.to_string()),
            details: None,
        }
    }

    /// The error envelope with details, for the contract tests (see
    /// [`RespData::err`]).
    #[cfg(test)]
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
    /// The stable error code the caller matches on (never the prose).
    code: String,
    /// Correlation id: matches the server-side log record that carries the
    /// full detail, and the `x-correlation-id` response header.
    correlation_id: String,
    /// Whether the same request may be retried as-is.
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    /// HTTP status the error is answered with, from the stable code.
    #[serde(skip)]
    status: StatusCode,
}

/// The HTTP status a stable code answers with.
fn status_of_code(code: &str) -> StatusCode {
    match code {
        "invalid_argument" => StatusCode::BAD_REQUEST,
        "not_found" => StatusCode::NOT_FOUND,
        "unauthenticated" => StatusCode::UNAUTHORIZED,
        "permission_denied" => StatusCode::FORBIDDEN,
        "conflict" => StatusCode::CONFLICT,
        "resource_exhausted" => StatusCode::TOO_MANY_REQUESTS,
        "unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        "deadline_exceeded" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// The deadline a route with no resolvable action profile runs under (the
/// engine's `default` class value; routes outside the action table — none
/// today — get the same bound).
pub const FALLBACK_DEADLINE_MS: u64 = 30_000;

impl AppError {
    /// Build from the shared external-error contract.
    pub fn from_external(error: acts::export::ExternalError) -> Self {
        Self {
            status: status_of_code(&error.code),
            code: error.code,
            message: error.message,
            correlation_id: error.correlation_id,
            retryable: error.retryable,
            details: None,
        }
    }

    /// The request itself is malformed or exceeds a server bound — a client
    /// fault, not a server failure.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::from_external(acts::export::ExternalError::new(
            acts::export::ExternalCode::Invalid,
            acts::export::safe_text(&message.into(), 1024),
            new_correlation_id(),
        ))
    }

    /// The request ran past its action profile's deadline. It gets its own
    /// stable code (`deadline_exceeded`), not a borrowed one: a client that
    /// timed out waiting must be able to tell "the server gave up" apart
    /// from "the server is shutting down" (`unavailable`).
    pub fn deadline_exceeded(message: impl Into<String>) -> Self {
        Self {
            status: status_of_code("deadline_exceeded"),
            code: "deadline_exceeded".to_string(),
            message: acts::export::safe_text(&message.into(), 1024),
            correlation_id: new_correlation_id(),
            retryable: true,
            details: None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({
            "code": "error",
            "ecode": self.code,
            "message": self.message,
            "correlation_id": self.correlation_id,
            "retryable": self.retryable,
            "details": self.details,
        });
        let mut response = (self.status, Json(body)).into_response();
        if let Ok(value) = axum::http::HeaderValue::from_str(&self.correlation_id) {
            response.headers_mut().insert(
                axum::http::header::HeaderName::from_static("x-correlation-id"),
                value,
            );
        }
        response
    }
}

impl From<&str> for AppError {
    fn from(value: &str) -> Self {
        Self::from_external(acts::export::ExternalError::new(
            acts::export::ExternalCode::Internal,
            acts::export::INTERNAL_MESSAGE,
            new_correlation_id(),
        ))
        .with_details(value)
    }
}

impl AppError {
    fn with_details(mut self, details: &str) -> Self {
        self.details = Some(acts::export::safe_text(details, 4096));
        self
    }
}

impl From<acts::ActError> for AppError {
    fn from(value: acts::ActError) -> Self {
        Self::from_external(acts::export::ExternalError::from_act(
            &value,
            &new_correlation_id(),
        ))
    }
}

impl From<acts::actions::Error> for AppError {
    fn from(value: acts::actions::Error) -> Self {
        Self::from_external(acts::export::ExternalError::from_action(
            &value,
            &new_correlation_id(),
        ))
    }
}

impl From<validator::ValidationErrors> for AppError {
    fn from(value: validator::ValidationErrors) -> Self {
        Self::bad_request(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error carrying details serializes them in the same envelope as the
    /// plain one, with the status and contract fields its code implies.
    #[test]
    fn app_error_with_details_answers_its_status() {
        let mut err = AppError::bad_request("page too deep");
        err.details = Some("the reason".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            response
                .headers()
                .contains_key(axum::http::header::HeaderName::from_static(
                    "x-correlation-id"
                )),
            "every error answers with its correlation id header"
        );
    }

    /// The stable codes map onto the HTTP statuses the caller matches on:
    /// budget refusals are 429, unavailable is 503, denial is 403.
    #[test]
    fn stable_codes_map_to_http_statuses() {
        use acts::export::{ExternalCode, ExternalError};
        let cases = [
            (ExternalCode::Invalid, StatusCode::BAD_REQUEST),
            (ExternalCode::Unauthenticated, StatusCode::UNAUTHORIZED),
            (ExternalCode::PermissionDenied, StatusCode::FORBIDDEN),
            (ExternalCode::Conflict, StatusCode::CONFLICT),
            (
                ExternalCode::ResourceExhausted,
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (ExternalCode::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
            (ExternalCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (code, status) in cases {
            let err = AppError::from_external(ExternalError::new(code, "m", "c"));
            assert_eq!(err.into_response().status(), status);
        }
    }
}
