//! HTTP authentication for the web plugin.
//!
//! The credential is the `authorization: Bearer <token>` request header — the
//! same one the gRPC transport reads from its metadata. Every route runs
//! through [`require_auth`], which resolves the header into an
//! [`acts::Principal`] and stashes it in the request extensions so the
//! handlers can hand it to `acts::actions::apply_as`. A request that cannot
//! be authenticated is answered before any handler runs.

use acts::{AclError, Engine};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
/// Resolve the bearer token into a principal, or answer 401.
pub async fn require_auth(
    State(engine): State<Arc<Engine>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|token| !token.is_empty());

    let principal = match engine.acl().authenticate(token) {
        Ok(principal) => principal,
        Err(err) => {
            let status = match err {
                AclError::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
                AclError::Denied(_) => StatusCode::FORBIDDEN,
            };
            return (status, err.to_string()).into_response();
        }
    };

    request.extensions_mut().insert(principal);
    next.run(request).await
}
