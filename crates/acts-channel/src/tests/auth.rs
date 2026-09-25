//! The user model over the transport: what a login answers with, and what
//! happens when the session it answered with goes stale.
//!
//! These cases run against a server whose ACL is enabled
//! ([`start_auth_server`](super::start_auth_server)) and declare their users
//! through `engine.acl().set_user(…)`: the config file no longer carries
//! roles or tokens, so a credential is only ever a login's answer.

use super::{server_addr, start_auth_server};
use crate::{ActsChannel, SessionTokens, Vars};
use acts::{Engine, UserSpec};
use acts_acl::ACCESS_TOKEN_TTL_SECS;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tonic::Code;

/// A server with a `reader` user (password `s3cret`) allowed the catalogue
/// reads — `@read` — and a handle that shuts it down.
async fn auth_server() -> (Engine, u16, oneshot::Sender<()>) {
    let (tx, rx) = oneshot::channel();
    let (engine, port) = start_auth_server(rx).await;
    engine
        .acl()
        .set_user(&UserSpec {
            name: "reader".to_string(),
            add_passwords: vec!["s3cret".to_string()],
            allow: Some(vec!["@read".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    (engine, port, tx)
}

fn url(port: u16) -> String {
    format!("http://{}:{port}", server_addr())
}

/// One login: the session it answers with is what every following request
/// presents, and a logout is what revokes it.
#[tokio::test]
async fn login_carries_a_session_and_logout_revokes_it() {
    let (_engine, port, tx) = auth_server().await;
    let url = url(port);

    let mut client = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    assert_eq!(
        client.url(),
        url,
        "the channel reports the endpoint it dialled"
    );

    let session = client.session().expect("a login answers a session");
    assert_eq!(client.token().as_deref(), Some(session.token.as_str()));
    // the expiries are the server's TTLs stamped on the local clock
    let now = chrono::Utc::now().timestamp_millis();
    assert!(
        session.expires_at > now && session.expires_at <= now + ACCESS_TOKEN_TTL_SECS * 1000,
        "the access token's expiry is not within its TTL: {session:?}"
    );
    assert!(
        session.refresh_expires_at > session.expires_at,
        "the refresh token must outlive the access token: {session:?}"
    );
    assert!(!session.is_expired() && !session.is_refresh_expired());

    // the token is the credential the server resolves
    let who: Value = client
        .send("acl:whoami", Vars::new())
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(who["user"], "reader");
    assert_eq!(who["authenticated"], json!(true));

    // what `@read` grants is served; a write is refused as a denial, not as a
    // missing credential
    assert!(client.send::<Value>("model:ls", Vars::new()).await.is_ok());
    let err = client
        .send::<()>("model:deploy", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    assert!(err.message().contains("reader"), "got: {}", err.message());

    // logout revokes the session and drops the credential in hand
    assert!(client.logout().await.unwrap());
    assert!(client.token().is_none());
    assert!(client.session().is_none());

    // the revoked token is anonymous again: the catalogue reads, nothing else
    let mut revoked = ActsChannel::connect_with_token(&url, Some(session.token.clone()))
        .await
        .unwrap();
    let who: Value = revoked
        .send("acl:whoami", Vars::new())
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(who["authenticated"], json!(false));
    assert!(revoked.send::<Value>("model:ls", Vars::new()).await.is_ok());
    let err = revoked
        .send::<()>("proc:ls", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    tx.send(()).unwrap();
}

/// A bad password is refused with the server's own message, and the answer
/// does not say which half of the pair was wrong.
#[tokio::test]
async fn a_wrong_password_is_refused_with_the_server_message() {
    let (_engine, port, tx) = auth_server().await;
    let url = url(port);

    // the direct call keeps the transport error, message and code alike
    let mut client = ActsChannel::connect(&url).await.unwrap();
    let err = client.login("reader", "wrong").await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    assert_eq!(err.message(), "invalid user or password");

    // and `connect_with_password` surfaces the same refusal to its caller
    let err = ActsChannel::connect_with_password(&url, "reader", "wrong")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid user or password"),
        "got: {err}"
    );

    // an unknown user reads exactly like a wrong password
    let err = ActsChannel::connect_with_password(&url, "nobody", "s3cret")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid user or password"),
        "got: {err}"
    );
    tx.send(()).unwrap();
}

/// A session whose access token is dead but whose refresh token lives — what a
/// persisted session reaches after a while — is rotated on the first request
/// and the request is retried, so the caller sees the answer.
#[tokio::test]
async fn a_stale_access_token_is_refreshed_transparently() {
    let (_engine, port, tx) = auth_server().await;
    let url = url(port);

    let mut probe = ActsChannel::connect(&url).await.unwrap();
    let live = probe.login("reader", "s3cret").await.unwrap();

    let stale = SessionTokens {
        token: "at_expired".to_string(),
        refresh_token: live.refresh_token.clone(),
        expires_at: chrono::Utc::now().timestamp_millis() - 1,
        refresh_expires_at: live.refresh_expires_at,
    };
    assert!(stale.is_expired() && !stale.is_refresh_expired());

    let mut client = ActsChannel::connect_with_session(&url, stale.clone())
        .await
        .unwrap();
    assert_eq!(client.token().as_deref(), Some("at_expired"));

    // `proc:ls` needs a credential, so the dead token is refused and the
    // session in hand is rotated before the request is retried
    let page: Value = client
        .send("proc:ls", Vars::new())
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(page["count"], json!(0));

    let rotated = client.session().expect("the session in hand was rotated");
    assert_ne!(rotated.token, "at_expired");
    assert_ne!(
        rotated.refresh_token, stale.refresh_token,
        "a refresh rotates the pair"
    );
    assert!(!rotated.is_expired() && client.token().as_deref() == Some(rotated.token.as_str()));
    tx.send(()).unwrap();
}

/// A session whose refresh token the server no longer knows cannot be revived:
/// the refresh's own refusal is what the caller sees, not the original one.
#[tokio::test]
async fn a_spent_refresh_token_is_reported_by_the_refresh() {
    let (_engine, port, tx) = auth_server().await;
    let url = url(port);
    let now = chrono::Utc::now().timestamp_millis();

    let stale = SessionTokens {
        token: "at_expired".to_string(),
        refresh_token: "rt_unknown".to_string(),
        expires_at: now - 1,
        refresh_expires_at: now + 60_000,
    };
    let mut client = ActsChannel::connect_with_session(&url, stale)
        .await
        .unwrap();

    let err = client.send::<()>("proc:ls", Vars::new()).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    assert!(
        err.message().contains("refresh token is unknown"),
        "got: {}",
        err.message()
    );
    tx.send(()).unwrap();
}

/// The two expiry questions a stored session is decided by: whether only the
/// access token needs rotating, and whether the session is dead outright.
#[test]
fn session_expiry_flags_follow_the_wall_clock() {
    let far = chrono::Utc::now().timestamp_millis() + 60_000;
    let past = chrono::Utc::now().timestamp_millis() - 1;
    let live = SessionTokens {
        token: "at".to_string(),
        refresh_token: "rt".to_string(),
        expires_at: far,
        refresh_expires_at: far,
    };
    assert!(!live.is_expired() && !live.is_refresh_expired());

    // an expired access token with a live refresh token is still rotatable
    let rotatable = SessionTokens {
        expires_at: past,
        ..live.clone()
    };
    assert!(rotatable.is_expired() && !rotatable.is_refresh_expired());

    // a spent refresh token makes the whole session dead
    let spent = SessionTokens {
        refresh_expires_at: past,
        ..live
    };
    assert!(spent.is_refresh_expired());
}
