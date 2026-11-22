//! HTTP authentication for the web plugin.
//!
//! The credential is the `authorization: Bearer <token>` request header — the
//! same one the gRPC transport reads from its metadata. Every route runs
//! through [`require_auth`], which resolves the header into an
//! [`acts::Principal`] and stashes it in the request extensions so the
//! handlers can hand it to `acts::actions::apply_as`. A request that cannot
//! be authenticated is answered before any handler runs.

use crate::objects::AppError;
use acts::{AuthenticationResult, Engine};
use axum::{
    extract::{Request, State},
    http::header::AUTHORIZATION,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// The longest credential accepted. A `Bearer <token>` header is tens of
/// bytes; anything past this is payload smuggled into a header (header
/// parsing cost, log size), refused before the token is even split.
const MAX_AUTHORIZATION_LEN: usize = 8 * 1024;

/// The `401` answer of the credential hardening, through the shared error
/// contract.
fn unauthorized(message: &'static str) -> Response {
    AppError::from_external(acts::ExternalError::new(
        acts::ExternalCode::Unauthenticated,
        message,
        acts::new_correlation_id(),
    ))
    .into_response()
}

/// Count one rejected credential against `acts_auth_invalid_total{transport}`.
fn count_invalid(engine: &Engine) {
    engine.ingress().metrics.incr(
        "acts_auth_invalid_total",
        &[("transport", acts::TRANSPORT_WEB)],
    );
}

/// Resolve the bearer token into a principal, or answer 401.
///
/// The three states are kept apart on purpose: no credential proceeds as the
/// anonymous caller, which holds no grant — the route's action check refuses
/// it (401) — a credential that does not check out is refused with 401 before
/// any handler runs, never folded into the anonymous caller, and a live
/// session proceeds as its principal.
///
/// Two shapes are refused before the credential is even resolved: more than
/// one `Authorization` header (two credentials at once is either an smuggling
/// attempt or a broken client — never a valid identity), and a header value
/// past [`MAX_AUTHORIZATION_LEN`]. Neither the token nor any part of it is
/// ever logged — a credential that ends up in a log line is a leaked
/// credential.
pub async fn require_auth(
    State(engine): State<Arc<Engine>>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut credentials = request.headers().get_all(AUTHORIZATION).iter();
    let (first, second) = (credentials.next(), credentials.next());
    if second.is_some() {
        count_invalid(&engine);
        return unauthorized(
            "unauthenticated: more than one authorization header is not a valid credential",
        );
    }
    let raw = match first {
        Some(value) if value.len() > MAX_AUTHORIZATION_LEN => {
            count_invalid(&engine);
            return unauthorized(
                "unauthenticated: the authorization header exceeds the accepted length",
            );
        }
        Some(value) => value.to_str().ok().map(str::to_owned),
        None => None,
    };

    let token = raw
        .as_deref()
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|token| !token.is_empty());

    let principal = match engine.acl().resolve(token).await {
        AuthenticationResult::Authenticated(principal) => principal,
        AuthenticationResult::MissingCredentials => engine.acl().anonymous(),
        AuthenticationResult::InvalidCredentials => {
            count_invalid(&engine);
            return unauthorized("unauthenticated: the credential is invalid or expired");
        }
    };

    request.extensions_mut().insert(principal);
    next.run(request).await
}
