//! The `auth` command group and the session the CLI keeps between runs.
//!
//! One case in its own test binary: the session file's directory comes from
//! `ACTS_CONFIG_DIR`, and `std::env::set_var` is unsafe in edition 2024, so it
//! is set here — before the runtime, and with it every thread that could read
//! it, exists — and no other case in this process can race it. The `auth`
//! commands then write into a scratch directory instead of the developer's own
//! `~/.acts`.

mod common;

use acts::UserSpec;
use acts_cli::{client, cmd::CommandRunner, session};
use anyhow::Context;
use std::path::PathBuf;

/// A scratch directory this case owns, empty at the start.
fn scratch(name: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("acts-cli-session-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn the_auth_commands_login_store_the_session_and_log_out() {
    let dir = scratch("auth").expect("a scratch config directory");
    // SAFETY: set before the runtime exists, so no thread can be reading the
    // environment concurrently; this binary holds this one case.
    unsafe { std::env::set_var(session::CONFIG_DIR_ENV, &dir) };

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a test runtime")
        .block_on(async {
            // the closure below is `move`, so it takes an owned copy: the
            // scratch directory itself stays here for the cleanup below
            let dir = dir.clone();
            common::with_server("auth", false, |engine, url| async move {
                engine
                    .acl()
                    .set_user(&UserSpec {
                        name: "reader".to_string(),
                        add_passwords: vec!["s3cret".to_string()],
                        allow: Some(vec!["@read".to_string()]),
                        ..Default::default()
                    })
                    .await?;

                assert_eq!(
                    session::config_dir(),
                    dir,
                    "the CLI reads its state directory from the environment"
                );

                // `auth login` swaps the connection's credential and stores the
                // session, so the next run against this server reuses it
                let mut channel = client::connect(&url, None).await?;
                let mut cli = CommandRunner::new(&mut channel);
                cli.run("auth login reader --password s3cret").await?;

                let stored =
                    session::load(&url).expect("the login stored a session for this server");
                assert_eq!(stored.user, "reader");
                assert_eq!(stored.server, url);

                // `auth whoami` prints the identity the server resolved
                cli.run("auth whoami").await?;
                assert_eq!(
                    cli.client().token().as_deref(),
                    Some(stored.tokens.token.as_str()),
                    "the token the login answered with is the one presented"
                );
                assert!(
                    dir.join("session.json").exists(),
                    "the session must be on disk for the next run"
                );

                // a fresh connection with no credential picks the stored
                // session up: the CLI's whole startup path
                let (mut reused, who) =
                    client::connect_and_authenticate(&url, None, None, None).await?;
                assert_eq!(client::identity(&who), "reader");
                assert!(
                    client::whoami(&mut reused).await?.is_object(),
                    "the reused session authenticates"
                );

                // a wrong password is the server's refusal, reported as the
                // action the user typed
                let err = cli
                    .run("auth login reader --password wrong")
                    .await
                    .unwrap_err();
                let text = format!("{err:#}");
                assert!(text.contains("acl:login"), "unexpected error: {text}");
                assert!(
                    text.contains("invalid user or password"),
                    "a wrong password must carry the server's message: {text}"
                );

                // `auth logout` drops the credential and forgets the session
                cli.run("auth logout").await?;
                assert!(cli.client().token().is_none());
                assert!(
                    session::load_from(&dir, &url).is_none(),
                    "logout must remove the stored session"
                );
                assert!(!dir.join("session.json").exists());

                // the `--user` startup path: with no session to reuse, the
                // credentials the caller passed are a login, and it is stored
                let (mut fresh, who) = client::connect_and_authenticate(
                    &url,
                    None,
                    Some("reader".to_string()),
                    Some("s3cret".to_string()),
                )
                .await?;
                assert_eq!(client::identity(&who), "reader");
                assert!(client::whoami(&mut fresh).await?.is_object());
                assert!(session::load_from(&dir, &url).is_some());
                session::clear_from(&dir, &url)?;

                // the session store itself, driven with an explicit directory:
                // a session is scoped to its server, and a spent one is
                // discarded on the way out
                let other = scratch("other").expect("a second scratch directory");
                let now = chrono::Utc::now().timestamp_millis();
                let tokens = acts_channel::SessionTokens {
                    token: "at_x".to_string(),
                    refresh_token: "rt_x".to_string(),
                    expires_at: now + 60_000,
                    refresh_expires_at: now + 60_000,
                };
                session::save_to(&other, &url, "reader", &tokens)?;
                let loaded = session::load_from(&other, &url).expect("the session round trips");
                assert_eq!(loaded.tokens.token, "at_x");
                assert!(
                    session::load_from(&other, "http://elsewhere:1").is_none(),
                    "another server is never handed this session"
                );
                session::save_to(&other, &url, "reader", &tokens)?;
                session::clear_from(&other, &url)?;
                assert!(session::load_from(&other, &url).is_none());

                let spent = acts_channel::SessionTokens {
                    refresh_expires_at: now - 1,
                    ..tokens
                };
                session::save_to(&other, &url, "reader", &spent)?;
                assert!(
                    session::load_from(&other, &url).is_none(),
                    "a session whose refresh token is spent is discarded"
                );
                assert!(
                    !other.join("session.json").exists(),
                    "the discarded session is removed"
                );

                let _ = std::fs::remove_dir_all(&other);
                Ok(())
            })
            .await
            .context("the session cases must reach a live server")
        })
        .expect("the auth commands must answer");

    let _ = std::fs::remove_dir_all(&dir);
}
