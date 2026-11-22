//! The one external error contract for every transport.
//!
//! A caller may depend on a stable `code` (and `retryable`) — never on the
//! prose of a message. Internal failures answer a fixed generic message
//! here; their full detail goes to the structured log under the same
//! correlation id, so an operator can match a caller's `correlation_id` to
//! the server-side cause without the caller ever seeing a store path, a SQL
//! fragment or a shell stderr.
//!
//! Web, gRPC and NATS all map their failures through [`ExternalError`], so
//! the same fault answers the same code on every transport (a database
//! hiccup is `internal` everywhere; a scheduler at capacity is
//! `resource_exhausted` everywhere).

use std::fmt;

/// The stable, versioned error codes a transport may answer with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalCode {
    /// The request itself is malformed (bad action name, missing field,
    /// oversized payload, undecodable data).
    Invalid,
    /// The addressed entity does not exist (or may not be named).
    NotFound,
    /// No credential, or one that selects no role.
    Unauthenticated,
    /// A credential without the right for the operation or scope.
    PermissionDenied,
    /// A conditional write lost its race; re-read and retry.
    Conflict,
    /// An admission bound refused the request (inflight, subscriptions,
    /// parse pressure, rate).
    ResourceExhausted,
    /// The engine cannot take work right now (shutting down, quiescing,
    /// lease lost).
    Unavailable,
    /// The engine failed internally; the message is fixed, the detail is
    /// logged server-side under the correlation id.
    Internal,
}

impl ExternalCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExternalCode::Invalid => "invalid_argument",
            ExternalCode::NotFound => "not_found",
            ExternalCode::Unauthenticated => "unauthenticated",
            ExternalCode::PermissionDenied => "permission_denied",
            ExternalCode::Conflict => "conflict",
            ExternalCode::ResourceExhausted => "resource_exhausted",
            ExternalCode::Unavailable => "unavailable",
            ExternalCode::Internal => "internal",
        }
    }
}

impl fmt::Display for ExternalCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The safe face of one failure: what a transport serializes to its caller.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalError {
    /// The stable code (see [`ExternalCode`]).
    pub code: String,
    /// A bounded, control-character-free message. For `internal` this is
    /// always the fixed phrase — the cause is server-side only.
    pub message: String,
    /// The correlation id the request entered with; matches the
    /// server-side log record that carries the full detail.
    pub correlation_id: String,
    /// Whether the caller may retry the same request as-is.
    pub retryable: bool,
}

impl ExternalError {
    pub fn new(
        code: ExternalCode,
        message: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Self {
        Self {
            code: code.as_str().to_string(),
            message: message.into(),
            correlation_id: correlation_id.into(),
            retryable: matches!(
                code,
                ExternalCode::Conflict | ExternalCode::ResourceExhausted
            ),
        }
    }

    /// Map an action-dispatch failure onto the external contract.
    ///
    /// `internal` (engine/store faults) never carries its message to the
    /// caller: the detail is logged here under the correlation id, and the
    /// caller sees the fixed phrase. Every other kind keeps its (bounded,
    /// sanitized) message — those texts are written for the caller ("`pid`
    /// is required", "needs a credential").
    pub fn from_action(err: &crate::export::actions::Error, correlation_id: &str) -> Self {
        use crate::export::actions::Error as ActionError;
        let (code, message) = match err {
            ActionError::NotFound(msg) => (ExternalCode::NotFound, safe_text(msg, 256)),
            ActionError::Invalid(msg) => (ExternalCode::Invalid, safe_text(msg, 1024)),
            ActionError::Unauthenticated(msg) => {
                (ExternalCode::Unauthenticated, safe_text(msg, 256))
            }
            ActionError::Denied(msg) => (ExternalCode::PermissionDenied, safe_text(msg, 256)),
            ActionError::Conflict(msg) => (ExternalCode::Conflict, safe_text(msg, 256)),
            ActionError::ResourceExhausted(msg) => {
                (ExternalCode::ResourceExhausted, safe_text(msg, 256))
            }
            ActionError::Unavailable(msg) => (ExternalCode::Unavailable, safe_text(msg, 256)),
            ActionError::Internal(msg) => {
                tracing::error!(
                    correlation_id,
                    detail = %safe_text(msg, 4096),
                    "internal error"
                );
                (ExternalCode::Internal, INTERNAL_MESSAGE.to_string())
            }
        };
        Self {
            code: code.as_str().to_string(),
            message,
            correlation_id: correlation_id.to_string(),
            retryable: matches!(
                code,
                ExternalCode::Conflict | ExternalCode::ResourceExhausted
            ),
        }
    }

    /// Map an engine error (middleware, transport plumbing, engine start)
    /// onto the external contract, with the same internal-detail rule.
    pub fn from_act(err: &crate::ActError, correlation_id: &str) -> Self {
        use crate::ActError as EngineError;
        let (code, message) = match err {
            EngineError::Unauthenticated(msg) => {
                (ExternalCode::Unauthenticated, safe_text(msg, 256))
            }
            EngineError::Denied(msg) => (ExternalCode::PermissionDenied, safe_text(msg, 256)),
            EngineError::Conflict(msg) => (ExternalCode::Conflict, safe_text(msg, 256)),
            EngineError::QueueFull => (
                ExternalCode::ResourceExhausted,
                "the scheduler queue is full".to_string(),
            ),
            EngineError::Shutdown => (
                ExternalCode::Unavailable,
                "the engine is shutting down".to_string(),
            ),
            EngineError::LeaseLost => (
                ExternalCode::Unavailable,
                "the database lease was lost".to_string(),
            ),
            other => {
                tracing::error!(
                    correlation_id,
                    detail = %safe_text(&other.to_string(), 4096),
                    "internal error"
                );
                (ExternalCode::Internal, INTERNAL_MESSAGE.to_string())
            }
        };
        Self {
            code: code.as_str().to_string(),
            message,
            correlation_id: correlation_id.to_string(),
            retryable: matches!(
                code,
                ExternalCode::Conflict | ExternalCode::ResourceExhausted
            ),
        }
    }

    /// The JSON face gRPC embeds in its status details and NATS puts in its
    /// reply fields.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.code,
            "message": self.message,
            "correlation_id": self.correlation_id,
            "retryable": self.retryable,
        })
    }
}

/// The fixed message an `internal` failure answers with.
pub const INTERNAL_MESSAGE: &str = "internal error; quote the correlation id to the operator";

tokio::task_local! {
    /// The correlation id of the request the current task serves. A
    /// transport scopes a request with [`scoped`]; every error built inside
    /// (through [`new_correlation_id`]) then answers that id, so the body,
    /// the transport's header and the server-side log record all name one
    /// request.
    static REQUEST_CORRELATION: String;
}

/// Run `future` as the request identified by `correlation_id`.
pub async fn scoped<F: Future>(correlation_id: String, future: F) -> F::Output {
    REQUEST_CORRELATION.scope(correlation_id, future).await
}

/// A correlation id for one error face: the request's id when the task runs
/// inside [`scoped`], else a fresh 16-character id.
pub fn new_correlation_id() -> String {
    REQUEST_CORRELATION
        .try_with(|cid| cid.clone())
        .unwrap_or_else(|_| nanoid::nanoid!(16))
}

/// Bound a string and strip control characters: a caller-supplied fragment
/// travels through error messages (a missing `pid`, a denied action name),
/// and neither a `\n` (log forging), an ESC (terminal control) nor a
/// one-megabyte echo belongs there. Newlines become spaces; every other C0
/// byte and C1 escape is dropped. Long inputs truncate at a char boundary
/// with a `…` marker.
pub fn safe_text(text: &str, max_chars: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .take(max_chars.saturating_sub(1))
        .collect();
    if text.chars().count() > max_chars.saturating_sub(1) {
        let mut out = cleaned;
        out.push('…');
        out
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::actions::Error as ActionError;

    /// Every non-internal kind keeps its caller-facing text; internal is
    /// fixed and its detail only reaches the (subscribed) log.
    #[test]
    fn action_errors_map_to_stable_codes() {
        let cases: Vec<(ActionError, &str, bool)> = vec![
            (
                ActionError::NotFound("proc:ghost".into()),
                "not_found",
                false,
            ),
            (
                ActionError::Invalid("`pid` is required".into()),
                "invalid_argument",
                false,
            ),
            (
                ActionError::Unauthenticated("no token".into()),
                "unauthenticated",
                false,
            ),
            (
                ActionError::Denied("needs write".into()),
                "permission_denied",
                false,
            ),
            (ActionError::Conflict("row moved".into()), "conflict", true),
            (
                ActionError::ResourceExhausted("too many in flight".into()),
                "resource_exhausted",
                true,
            ),
            (
                ActionError::Unavailable("shutting down".into()),
                "unavailable",
                false,
            ),
        ];
        for (err, code, retryable) in cases {
            let external = ExternalError::from_action(&err, "cid-1");
            assert_eq!(external.code, code, "{err}");
            assert_eq!(external.retryable, retryable, "{err}");
            assert_eq!(external.correlation_id, "cid-1");
        }
    }

    /// An internal failure answers the fixed phrase and logs its detail
    /// under the correlation id (the log record itself is asserted by the
    /// grpc plugin's coverage suite; here the contract is the message).
    #[test]
    fn internal_errors_never_carry_their_detail() {
        let err = ActionError::Internal(
            "failed to create store directory /srv/acts/data: permission denied".into(),
        );
        let external = ExternalError::from_action(&err, "cid-2");
        assert_eq!(external.code, "internal");
        assert_eq!(external.message, INTERNAL_MESSAGE);
        assert!(!external.retryable);
        assert!(!external.message.contains("/srv/acts/data"));
    }

    /// The same rule for engine errors: queue pressure and shutdown map to
    /// the budget codes, everything else is internal.
    #[test]
    fn act_errors_map_to_stable_codes() {
        assert_eq!(
            ExternalError::from_act(&crate::ActError::QueueFull, "c").code,
            "resource_exhausted"
        );
        assert_eq!(
            ExternalError::from_act(&crate::ActError::Shutdown, "c").code,
            "unavailable"
        );
        assert_eq!(
            ExternalError::from_act(&crate::ActError::LeaseLost, "c").code,
            "unavailable"
        );
        let store = ExternalError::from_act(
            &crate::ActError::Store("sled path /var/lib/acts unreadable".into()),
            "c",
        );
        assert_eq!(store.code, "internal");
        assert!(!store.message.contains("/var/lib/acts"));
    }

    /// `safe_text` forges nothing into a log line, hides no terminal escape
    /// and bounds the echo.
    #[test]
    fn safe_text_bounds_and_neutralizes() {
        assert_eq!(safe_text("pid\nERROR fake", 256), "pid ERROR fake");
        assert_eq!(safe_text("a\u{1b}[31mred", 256), "a\u{fffd}[31mred");
        assert_eq!(safe_text("tab\tkept", 256), "tab kept");
        assert_eq!(safe_text("short", 256), "short");

        let long = "x".repeat(300);
        let bounded = safe_text(&long, 100);
        assert_eq!(bounded.chars().count(), 100);
        assert!(bounded.ends_with('…'));
        // a multi-byte char is never split
        let wide = "宽".repeat(50);
        assert_eq!(safe_text(&wide, 10).chars().count(), 10);
    }

    /// The JSON face carries exactly the four contract fields.
    #[test]
    fn external_error_json_shape() {
        let err = ExternalError::new(ExternalCode::ResourceExhausted, "busy", "cid-9");
        let json = err.to_json();
        assert_eq!(json["code"], "resource_exhausted");
        assert_eq!(json["message"], "busy");
        assert_eq!(json["correlation_id"], "cid-9");
        assert_eq!(json["retryable"], true);
    }

    /// Every code has one stable string, and `new` sets `retryable` by kind.
    #[test]
    fn every_code_is_stable_and_retryability_is_by_kind() {
        use ExternalCode::*;
        for (code, text, retryable) in [
            (Invalid, "invalid_argument", false),
            (NotFound, "not_found", false),
            (Unauthenticated, "unauthenticated", false),
            (PermissionDenied, "permission_denied", false),
            (Conflict, "conflict", true),
            (ResourceExhausted, "resource_exhausted", true),
            (Unavailable, "unavailable", false),
            (Internal, "internal", false),
        ] {
            assert_eq!(code.as_str(), text);
            assert_eq!(code.to_string(), text);
            assert_eq!(ExternalError::new(code, "m", "c").retryable, retryable);
        }
    }

    /// The remaining engine-error kinds map onto the same contract.
    #[test]
    fn act_error_variants_map_to_codes() {
        for (err, code) in [
            (
                crate::ActError::Unauthenticated("u".into()),
                "unauthenticated",
            ),
            (crate::ActError::Denied("d".into()), "permission_denied"),
            (crate::ActError::Conflict("c".into()), "conflict"),
        ] {
            let external = ExternalError::from_act(&err, "cid");
            assert_eq!(external.code, code, "{err}");
            assert_eq!(external.correlation_id, "cid");
        }
    }

    /// `safe_text` edges: an empty string, a zero budget (the marker alone),
    /// and a boundary exactly at the limit (no marker).
    #[test]
    fn safe_text_edges() {
        assert_eq!(safe_text("", 10), "");
        assert_eq!(safe_text("abc", 0), "…");
        // The budget includes the truncation marker: a budget of 3 keeps
        // two characters and the marker.
        assert_eq!(safe_text("abc", 3), "ab…");
        assert_eq!(safe_text("abc", 4), "abc");
        assert_eq!(safe_text("abcd", 3), "ab…");
    }
}
