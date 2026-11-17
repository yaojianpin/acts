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

    // a revoked token no longer resolves, so every action — the catalogue
    // reads included — is refused as an invalid credential; it never degrades
    // to the anonymous caller
    let mut revoked = ActsChannel::connect_with_token(&url, Some(session.token.clone()))
        .await
        .unwrap();
    for action in ["acl:whoami", "model:ls"] {
        let err = revoked
            .send::<Value>(action, Vars::new())
            .await
            .unwrap_err();
        assert_eq!(err.code(), Code::Unauthenticated, "{action}");
        assert!(
            err.message().contains("invalid or expired"),
            "{action}: got {}",
            err.message()
        );
    }

    // presenting no credential is no way in either: the anonymous caller may
    // run nothing but the auth actions
    let mut anonymous = ActsChannel::connect(&url).await.unwrap();
    let err = anonymous
        .send::<Value>("model:ls", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    assert!(
        err.message().contains("a session token is required"),
        "got: {}",
        err.message()
    );
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

/// A session that cannot be revived — its refresh is spent too — is repaired
/// by a fresh login, and the dead bearer in hand must not ride along with it:
/// the login's credential is its payload.
#[tokio::test]
async fn a_login_replaces_a_session_the_server_no_longer_resolves() {
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

    // a request that needs a credential is refused, and the refresh it
    // triggers is refused too: the session cannot be rotated back to life
    let err = client.send::<()>("proc:ls", Vars::new()).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);

    // the login answers a live session, and the channel presents that one
    let session = client.login("reader", "s3cret").await.unwrap();
    assert_eq!(client.token().as_deref(), Some(session.token.as_str()));
    let who: Value = client
        .send("acl:whoami", Vars::new())
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(who["user"], "reader");
    tx.send(()).unwrap();
}

/// One user may hold several sessions at once: two clients log in, and
/// logging one out leaves the other — its token and its refresh — untouched.
#[tokio::test]
async fn sessions_of_one_user_are_independent() {
    let (_engine, port, tx) = auth_server().await;
    let url = url(port);

    let mut laptop = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    let mut phone = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    assert_ne!(
        laptop.token(),
        phone.token(),
        "each login is its own session"
    );

    // both resolve to the same user
    for client in [&mut laptop, &mut phone] {
        let who: Value = client
            .send("acl:whoami", Vars::new())
            .await
            .unwrap()
            .data
            .unwrap();
        assert_eq!(who["user"], "reader");
    }

    // logging the laptop out revokes only its own session: the channel in
    // hand drops the credential (so it is the anonymous caller now), and the
    // token it held is refused when presented again
    let laptop_token = laptop.token().expect("the laptop holds a session");
    assert!(laptop.logout().await.unwrap());
    assert!(laptop.token().is_none());
    let mut revoked = ActsChannel::connect_with_token(&url, Some(laptop_token))
        .await
        .unwrap();
    let err = revoked
        .send::<Value>("acl:whoami", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    let who: Value = phone
        .send("acl:whoami", Vars::new())
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(who["user"], "reader");

    // and the phone's session still rotates on its own
    let before = phone.session().expect("the phone holds a session");
    let rotated = phone.refresh().await.unwrap();
    assert_ne!(rotated.refresh_token, before.refresh_token);
    assert!(phone.send::<Value>("model:ls", Vars::new()).await.is_ok());
    tx.send(()).unwrap();
}

/// A channel for an unrestricted administrator: the `root` user is declared
/// on `engine` and logged in on `port`.
async fn root_channel(engine: &Engine, port: u16) -> ActsChannel {
    engine
        .acl()
        .set_user(&UserSpec {
            name: "root".to_string(),
            add_passwords: vec!["root-pass".to_string()],
            allow: Some(vec!["*".to_string()]),
            patterns: Some(vec!["*".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    ActsChannel::connect_with_password(&url(port), "root", "root-pass")
        .await
        .unwrap()
}

/// An administrator's revocation reaches a caller that is already logged in:
/// `acl:logout_user` kills every session of the named user (and only that
/// user's), and `acl:deluser` cuts the user off outright.
#[tokio::test]
async fn an_admin_can_cut_a_logged_in_user_off() {
    let (engine, port, tx) = auth_server().await;
    let url = url(port);

    let mut admin = root_channel(&engine, port).await;
    let mut reader = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    assert!(
        reader
            .send::<Value>("acl:whoami", Vars::new())
            .await
            .is_ok()
    );

    // the administrator logs the reader out everywhere: its next request is
    // refused, while the administrator's own session keeps working
    let done: Value = admin
        .send("acl:logout_user", Vars::new().with("user", "reader"))
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(done, json!(true));
    let err = reader
        .send::<Value>("acl:whoami", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    assert!(admin.send::<Value>("acl:whoami", Vars::new()).await.is_ok());
    // an unknown name is a not-found, not a silent success
    let err = admin
        .send::<Value>("acl:logout_user", Vars::new().with("user", "nobody"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::NotFound);

    // the credentials were not touched: the reader logs in again
    let mut reader = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    assert!(reader.send::<Value>("model:ls", Vars::new()).await.is_ok());

    // deleting the user cuts the live session off, and a new login is refused
    let done: Value = admin
        .send("acl:deluser", Vars::new().with("user", "reader"))
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(done, json!(true));
    let err = reader
        .send::<Value>("acl:whoami", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    let err = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid user or password"),
        "got: {err}"
    );
    tx.send(()).unwrap();
}

/// An administrator can disable a user (its live sessions die and its login
/// is refused) and enable it again (the login reopens); a redundant enable
/// never disturbs a live session.
#[tokio::test]
async fn an_admin_can_disable_and_enable_a_user() {
    let (engine, port, tx) = auth_server().await;
    let url = url(port);

    let mut admin = root_channel(&engine, port).await;
    let mut reader = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    assert!(
        reader
            .send::<Value>("acl:whoami", Vars::new())
            .await
            .is_ok()
    );

    // disabling cuts the live session off and refuses the login
    let done: Value = admin
        .send("acl:disable_user", Vars::new().with("user", "reader"))
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(done, json!(true));
    let err = reader
        .send::<Value>("acl:whoami", Vars::new())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    let err = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid user or password"),
        "a disabled user must not log in: {err}"
    );

    // enabling reopens the login
    let done: Value = admin
        .send("acl:enable_user", Vars::new().with("user", "reader"))
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(done, json!(true));
    let mut reader = ActsChannel::connect_with_password(&url, "reader", "s3cret")
        .await
        .unwrap();
    assert!(reader.send::<Value>("model:ls", Vars::new()).await.is_ok());

    // a redundant enable leaves the live session alone, and an unknown name is
    // a not-found (like every other user action)
    let done: Value = admin
        .send("acl:enable_user", Vars::new().with("user", "reader"))
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(done, json!(true));
    assert!(
        reader
            .send::<Value>("acl:whoami", Vars::new())
            .await
            .is_ok()
    );
    let err = admin
        .send::<Value>("acl:disable_user", Vars::new().with("user", "nobody"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::NotFound);
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
