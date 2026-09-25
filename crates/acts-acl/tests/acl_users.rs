//! End-to-end access control: users, sessions, resource (`rn`) grants and
//! snapshot scope ownership, exercised through the public engine API.
//!
//! These cases go through `acts::actions::apply_as` — the same entry point
//! every transport uses — so what a gRPC or HTTP caller can do is what these
//! assert, without a socket in the way.

use acts::{Engine, UserSpec, Workflow, actions::apply_as};
use acts_acl::{ADMIN_USER, AclUsers};
use std::collections::HashMap;
use std::sync::Arc;

/// A model with a resource name (`rn`) and a single trivial step.
fn model(id: &str, rn: &str) -> String {
    format!(
        r#"
id: {id}
ver: 0.1.0
rn: {rn}
steps:
  - name: finish
"#
    )
}

fn vars_user(user: &str, password: &str) -> acts::Vars {
    acts::Vars::new()
        .with("user", user)
        .with("password", password)
}

/// Log in over the wire (the action a transport calls) and return the token.
async fn login(engine: &Engine, user: &str, password: &str) -> String {
    let value = apply_as(
        engine,
        &engine.anonymous(),
        "acl:login",
        vars_user(user, password),
    )
    .await
    .expect("login succeeds");
    value["token"]
        .as_str()
        .expect("a token is answered")
        .to_string()
}

async fn engine() -> Engine {
    // the store-backed registry is a separate crate: an engine runs it when it
    // is installed, and answers nobody's login without it
    Engine::builder()
        .with_user_acl()
        .start()
        .await
        .expect("the engine starts")
}

#[tokio::test]
async fn every_engine_bootstraps_the_builtin_admin() {
    let engine = engine().await;
    let names = engine.acl().user_names().await;
    assert!(
        names.contains(&ADMIN_USER.to_string()),
        "the builtin admin exists: {names:?}"
    );
    // ...and its password is unknown to a caller that guesses: the bootstrap
    // password comes from ACTS_ADMIN_PASSWORD or is generated and logged
    let err = engine.acl().login(ADMIN_USER, "not-the-password").await;
    assert!(err.is_err(), "a wrong admin password must not authenticate");
}

#[tokio::test]
async fn login_answers_a_session_that_authenticates_and_refreshes() {
    let engine = engine().await;
    engine
        .acl()
        .set_user(&UserSpec {
            name: "op".to_string(),
            add_passwords: vec!["op-pass".to_string()],
            allow: Some(vec!["@read".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();

    let value = apply_as(
        &engine,
        &engine.anonymous(),
        "acl:login",
        vars_user("op", "op-pass"),
    )
    .await
    .unwrap();
    let token = value["token"].as_str().unwrap().to_string();
    let refresh_token = value["refresh_token"].as_str().unwrap().to_string();
    assert!(value["expires_in"].as_i64().unwrap() > 0);

    // the token is a credential: whoami answers the user it belongs to
    let principal = engine.acl().authenticate(Some(&token)).unwrap();
    assert_eq!(principal.subject(), "op");
    assert!(principal.is_authenticated());
    let who = apply_as(&engine, &principal, "acl:whoami", acts::Vars::new())
        .await
        .unwrap();
    assert_eq!(who["user"], "op");

    // refresh rotates the pair and kills the old one
    let next = apply_as(
        &engine,
        &engine.anonymous(),
        "acl:refresh",
        acts::Vars::new().with("refresh_token", refresh_token.clone()),
    )
    .await
    .unwrap();
    let next_token = next["token"].as_str().unwrap().to_string();
    assert_ne!(next_token, token);
    assert!(
        !engine
            .acl()
            .authenticate(Some(&token))
            .unwrap()
            .is_authenticated()
    );
    assert!(
        engine
            .acl()
            .authenticate(Some(&next_token))
            .unwrap()
            .is_authenticated()
    );
    // the spent refresh token cannot be replayed
    let replay = apply_as(
        &engine,
        &engine.anonymous(),
        "acl:refresh",
        acts::Vars::new().with("refresh_token", refresh_token),
    )
    .await;
    assert!(replay.is_err(), "a refresh token is single-use");

    // logout revokes the live session
    let principal = engine.acl().authenticate(Some(&next_token)).unwrap();
    apply_as(
        &engine,
        &principal,
        "acl:logout",
        acts::Vars::new().with("token", next_token.clone()),
    )
    .await
    .unwrap();
    assert!(
        !engine
            .acl()
            .authenticate(Some(&next_token))
            .unwrap()
            .is_authenticated()
    );
}

#[tokio::test]
async fn a_session_survives_an_engine_restart() {
    let store = Arc::new(acts::MemoryStore::new());
    let engine = Engine::builder()
        .with_user_acl()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    engine
        .acl()
        .set_user(&UserSpec {
            name: "op".to_string(),
            add_passwords: vec!["op-pass".to_string()],
            allow: Some(vec!["@read".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    let token = login(&engine, "op", "op-pass").await;

    // a second engine over the same store reads the users and the session
    let restarted = Engine::builder()
        .with_user_acl()
        .set_store(store)
        .start()
        .await
        .unwrap();
    let principal = restarted.acl().authenticate(Some(&token)).unwrap();
    assert!(
        principal.is_authenticated(),
        "a persisted session authenticates after a restart"
    );
    assert_eq!(principal.subject(), "op");
    assert!(
        restarted
            .acl()
            .authenticate(Some(&login(&restarted, "op", "op-pass").await))
            .unwrap()
            .is_authenticated()
    );
}

#[tokio::test]
async fn a_resource_grant_bounds_deploy_and_start() {
    let engine = engine().await;
    engine
        .acl()
        .set_user(&UserSpec {
            name: "orders-op".to_string(),
            add_passwords: vec!["p".to_string()],
            // writes are allowed by command pattern; which resources they may
            // touch is a separate grant
            allow: Some(vec![
                "model:deploy".to_string(),
                "proc:start".to_string(),
                "@read".to_string(),
            ]),
            patterns: Some(vec!["orders:*".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    let token = login(&engine, "orders-op", "p").await;
    let principal = engine.acl().authenticate(Some(&token)).unwrap();

    // the matching resource deploys and starts
    apply_as(
        &engine,
        &principal,
        "model:deploy",
        acts::Vars::new().with("model", model("m-orders", "orders:eu")),
    )
    .await
    .expect("a matching rn deploys");

    apply_as(
        &engine,
        &principal,
        "proc:start",
        acts::Vars::new().with("id", "m-orders"),
    )
    .await
    .expect("a matching rn starts");

    // another resource is refused, deploy and start alike
    let denied = apply_as(
        &engine,
        &principal,
        "model:deploy",
        acts::Vars::new().with("model", model("m-billing", "billing:eu")),
    )
    .await
    .expect_err("another resource is refused");
    assert!(
        matches!(denied, acts::actions::Error::Denied(_)),
        "{denied:?}"
    );

    // so is a model that claims no resource at all
    let denied = apply_as(
        &engine,
        &principal,
        "model:deploy",
        acts::Vars::new().with("model", model("m-bare", "")),
    )
    .await
    .expect_err("an rn-less model needs an unrestricted user");
    assert!(
        matches!(denied, acts::actions::Error::Denied(_)),
        "{denied:?}"
    );

    // an invalid rn is a model error, not a grant decision
    let invalid = apply_as(
        &engine,
        &principal,
        "model:deploy",
        acts::Vars::new().with("model", model("m-bad", "orders:*")),
    )
    .await
    .expect_err("a glob is not a resource name");
    assert!(
        matches!(invalid, acts::actions::Error::Invalid(_)),
        "{invalid:?}"
    );
}

#[tokio::test]
async fn catalog_groups_gate_whole_command_classes() {
    let engine = engine().await;
    engine
        .acl()
        .set_user(&UserSpec {
            name: "reader".to_string(),
            add_passwords: vec!["p".to_string()],
            allow: Some(vec!["@read".to_string()]),
            patterns: Some(vec!["*".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();

    // a user that grants @read deploys through a command pattern instead
    let denied = apply_as(
        &engine,
        &engine
            .acl()
            .authenticate(Some(&login(&engine, "reader", "p").await))
            .unwrap(),
        "model:deploy",
        acts::Vars::new().with("model", model("m1", "orders:eu")),
    )
    .await
    .expect_err("@read does not deploy");
    assert!(
        matches!(denied, acts::actions::Error::Denied(_)),
        "{denied:?}"
    );

    // ...and reads work
    let principal = engine
        .acl()
        .authenticate(Some(&login(&engine, "reader", "p").await))
        .unwrap();
    apply_as(&engine, &principal, "model:ls", acts::Vars::new())
        .await
        .expect("@read lists models");
    let denied = apply_as(
        &engine,
        &principal,
        "model:rm",
        acts::Vars::new().with("id", "m1"),
    )
    .await
    .expect_err("@read does not remove");
    assert!(
        matches!(denied, acts::actions::Error::Denied(_)),
        "{denied:?}"
    );
}

#[tokio::test]
async fn snapshot_scopes_belong_to_their_subject() {
    let engine = engine().await;
    for (user, scope) in [("alice", "alice"), ("bob", "bob")] {
        engine
            .acl()
            .set_user(&UserSpec {
                name: user.to_string(),
                add_passwords: vec!["p".to_string()],
                allow: Some(vec![
                    "snap:upsert".to_string(),
                    "snap:get".to_string(),
                    "snap:ls".to_string(),
                ]),
                patterns: Some(vec!["*".to_string()]),
                snapshot: Some(HashMap::from([(
                    "secrets".to_string(),
                    vec!["$subject".to_string()],
                )])),
                ..Default::default()
            })
            .await
            .unwrap();
        let principal = engine
            .acl()
            .authenticate(Some(&login(&engine, user, "p").await))
            .unwrap();
        apply_as(
            &engine,
            &principal,
            "snap:upsert",
            acts::Vars::new()
                .with("name", "secrets")
                .with("scope", scope)
                .with("rev", 1u64)
                .with(
                    "data",
                    acts::Vars::new().with("TOKEN", format!("{scope}-secret")),
                ),
        )
        .await
        .expect("a subject writes its own scope");
    }

    let alice = engine
        .acl()
        .authenticate(Some(&login(&engine, "alice", "p").await))
        .unwrap();
    // alice reads her own scope...
    let own = apply_as(
        &engine,
        &alice,
        "snap:get",
        acts::Vars::new()
            .with("name", "secrets")
            .with("scope", "alice"),
    )
    .await
    .unwrap();
    assert_eq!(own["data"]["TOKEN"], "alice-secret");
    // ...but not bob's
    let denied = apply_as(
        &engine,
        &alice,
        "snap:get",
        acts::Vars::new()
            .with("name", "secrets")
            .with("scope", "bob"),
    )
    .await
    .expect_err("another subject's scope is not readable");
    assert!(
        matches!(denied, acts::actions::Error::Denied(_)),
        "{denied:?}"
    );

    // the listing shows only what she owns
    let listed = apply_as(
        &engine,
        &alice,
        "snap:ls",
        acts::Vars::new().with("name", "secrets"),
    )
    .await
    .unwrap();
    let scopes: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["scope"].as_str())
        .collect();
    assert_eq!(scopes, vec!["alice"]);
}

#[tokio::test]
async fn user_administration_is_an_admin_action() {
    let engine = engine().await;
    engine
        .acl()
        .set_user(&UserSpec {
            name: "op".to_string(),
            add_passwords: vec!["p".to_string()],
            allow: Some(vec!["@read".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();

    let op = engine
        .acl()
        .authenticate(Some(&login(&engine, "op", "p").await))
        .unwrap();
    // an ordinary user cannot hand itself grants
    let denied = apply_as(
        &engine,
        &op,
        "acl:setuser",
        acts::Vars::new().with("user", serde_json::json!({"name": "op", "allow": ["*"]})),
    )
    .await
    .expect_err("only an admin manages users");
    assert!(
        matches!(denied, acts::actions::Error::Denied(_)),
        "{denied:?}"
    );
    // ...and it cannot read the registry back beyond what @read covers:
    // `acl:users`/`acl:getuser` are reads, so they are allowed, while every
    // write to it is refused above
    assert!(
        apply_as(&engine, &op, "acl:users", acts::Vars::new())
            .await
            .is_ok()
    );
    assert!(
        apply_as(
            &engine,
            &op,
            "acl:deluser",
            acts::Vars::new().with("user", "op")
        )
        .await
        .is_err()
    );

    // the anonymous caller is refused as unauthenticated, not as denied
    let err = apply_as(
        &engine,
        &engine.anonymous(),
        "acl:setuser",
        acts::Vars::new().with("user", serde_json::json!({"name": "x"})),
    )
    .await
    .expect_err("anonymous cannot manage users");
    assert!(
        matches!(err, acts::actions::Error::Unauthenticated(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_admin_writes_users_over_the_wire() {
    let engine = engine().await;
    // take the builtin admin over with a password we know (its bootstrap one
    // is generated or comes from the environment)
    engine
        .acl()
        .set_user(&UserSpec {
            name: ADMIN_USER.to_string(),
            add_passwords: vec!["root-pass".to_string()],
            ..Default::default()
        })
        .await
        .unwrap();
    let admin = engine
        .acl()
        .authenticate(Some(&login(&engine, ADMIN_USER, "root-pass").await))
        .unwrap();

    apply_as(
        &engine,
        &admin,
        "acl:setuser",
        acts::Vars::new().with(
            "user",
            serde_json::json!({
                "name": "deployer",
                "add_passwords": ["dp"],
                "allow": ["model:deploy", "@read"],
                "patterns": ["app:*"],
                "snapshot": {"secrets": ["$subject"]},
            }),
        ),
    )
    .await
    .expect("the admin creates a user");

    let view = apply_as(
        &engine,
        &admin,
        "acl:getuser",
        acts::Vars::new().with("user", "deployer"),
    )
    .await
    .unwrap();
    assert_eq!(view["name"], "deployer");
    assert_eq!(view["patterns"][0], "app:*");
    assert_eq!(view["snapshot"]["secrets"][0], "$subject");

    let names = apply_as(&engine, &admin, "acl:users", acts::Vars::new())
        .await
        .unwrap();
    assert!(names.as_array().unwrap().iter().any(|n| n == "deployer"));

    // and the new user works immediately
    let deployer = engine
        .acl()
        .authenticate(Some(&login(&engine, "deployer", "dp").await))
        .unwrap();
    apply_as(
        &engine,
        &deployer,
        "model:deploy",
        acts::Vars::new().with("model", model("m-app", "app:billing")),
    )
    .await
    .expect("the new grants apply to a fresh session");

    // deleting a user revokes its sessions
    let token = login(&engine, "deployer", "dp").await;
    apply_as(
        &engine,
        &admin,
        "acl:deluser",
        acts::Vars::new().with("user", "deployer"),
    )
    .await
    .unwrap();
    assert!(
        !engine
            .acl()
            .authenticate(Some(&token))
            .unwrap()
            .is_authenticated()
    );
    assert!(engine.acl().login("deployer", "dp").await.is_err());
}

#[tokio::test]
async fn a_disabled_acl_passes_everyone_through() {
    let engine = Engine::builder().disable_acl().start().await.unwrap();
    let principal = engine.anonymous();
    assert!(principal.is_unrestricted());
    apply_as(
        &engine,
        &principal,
        "model:deploy",
        acts::Vars::new().with("model", model("m-any", "")),
    )
    .await
    .expect("a disabled ACL checks nothing");
}

#[tokio::test]
async fn a_stale_acl_config_section_is_ignored() {
    // the section no longer configures anything: a deployment that still
    // writes one keeps running, on the builtin user model
    let config: acts::Config = {
        let mut config = acts::Config::default();
        config.table.insert(
            "acl".to_string(),
            toml::from_str::<toml::Table>("enabled = true\ntoken = \"legacy\"")
                .unwrap()
                .into(),
        );
        config
    };
    let engine = Engine::builder()
        .with_user_acl()
        .set_config(&config)
        .start()
        .await
        .unwrap();
    // the legacy token is not a credential
    let principal = engine.acl().authenticate(Some("legacy")).unwrap();
    assert!(!principal.is_authenticated());
    // and the store's users are what count
    assert!(
        engine
            .acl()
            .user_names()
            .await
            .contains(&ADMIN_USER.to_string())
    );
    // the workflow type itself carries the resource name
    let wf = Workflow::from_yml(&model("m1", "orders:eu")).unwrap();
    assert_eq!(wf.rn, "orders:eu");
}
