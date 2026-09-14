//! Shared message-action dispatch for transport plugins and engine embedders.
//!
//! Every inbound channel message is a `name` plus a `Vars` payload. This
//! module maps the name to the matching engine operation (process/model/act/
//! msg/evt/snapshot) and returns the JSON-serialized result. Transport plugins
//! (`acts-plugin-grpc`, `acts-plugin-nats`, `acts-plugin-web`) authenticate the
//! request into a [`crate::Principal`] and call [`apply_as`]; [`apply`] is the
//! anonymous in-process entry. Every transport speaks one action set and the
//! table is maintained in a single place.
//!
//! Error kinds map to transport semantics:
//! - [`Error::NotFound`] — unknown action name (`not found`)
//! - [`Error::Invalid`] — malformed/missing payload fields (`invalid argument`)
//! - [`Error::Unauthenticated`] — no credential, or one selecting no role
//! - [`Error::Denied`] — a credential without the right for this action/scope
//! - [`Error::Internal`] — engine/store failure (`internal error`)

use crate::utils::consts;
use crate::{Engine, Vars, Workflow};
use serde_json::{Value as JsonValue, json};
use std::fmt;

/// Action dispatch failure.
#[derive(Debug)]
pub enum Error {
    /// Unknown action name.
    NotFound(String),
    /// Malformed or missing payload fields.
    Invalid(String),
    /// Engine/store failure.
    Internal(String),
    /// No token, or a token that selects no role, under an enabled ACL.
    Unauthenticated(String),
    /// Authenticated caller without the right to run the action (or to name
    /// the snapshot scope it targets).
    Denied(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(msg) => write!(f, "not found action '{msg}'"),
            Error::Invalid(msg) => f.write_str(msg),
            Error::Internal(msg) => f.write_str(msg),
            Error::Unauthenticated(msg) => write!(f, "unauthenticated: {msg}"),
            Error::Denied(msg) => write!(f, "permission denied: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Ret = std::result::Result<JsonValue, Error>;

/// Serialize an already-resolved engine result into its wire value.
fn value<T: serde::Serialize>(r: crate::Result<T>) -> Ret {
    serde_json::to_value(r.map_err(|e| Error::Internal(e.to_string()))?)
        .map_err(|e| Error::Internal(e.to_string()))
}

/// Map a unit engine result to the action protocol's `true` value.
fn unit_ok(r: crate::Result<()>) -> Ret {
    r.map(|_| json!(true))
        .map_err(|e| Error::Internal(e.to_string()))
}

fn pop(options: &mut Vars, key: &str) -> std::result::Result<String, Error> {
    options
        .pop::<String>(key)
        .ok_or_else(|| Error::Invalid(format!("{key} is required")))
}

/// Map an ACL refusal onto the action protocol's error kinds, so every
fn deny(err: crate::AclError) -> Error {
    match err {
        crate::AclError::Unauthenticated(msg) => Error::Unauthenticated(msg),
        crate::AclError::Denied(msg) => Error::Denied(msg),
    }
}

/// Apply a channel message action as an anonymous in-process caller.
///
/// Without an `[acl]` section this is the only entry point and nothing is
/// enforced. With one, the caller resolves to the configured `default_role`
/// (or is refused outright when none is configured), so an embedder that
/// never carries a token cannot sidestep the transport checks.
pub async fn apply(engine: &Engine, name: &str, options: Vars) -> Ret {
    let principal = engine.acl().anonymous();
    apply_as(engine, &principal, name, options).await
}

/// Apply a channel message action on behalf of an authenticated principal.
///
/// `name` selects the operation; `options` is the payload. On success the
/// returned value serializes exactly like the old gRPC `Message.data`.
///
/// Two checks precede the dispatch: the action itself must be allowed, and
/// every snapshot scope the action names (or would return) must belong to the
/// principal's subject — see [`crate::acl`].
pub async fn apply_as(
    engine: &Engine,
    principal: &crate::Principal,
    name: &str,
    mut options: Vars,
) -> Ret {
    // `acl:whoami` is implicitly allowed — but only once a token resolved, so
    // it doubles as a startup check without being reachable anonymously.
    if name == crate::acl::ACTION_WHOAMI {
        return Ok(principal.to_value());
    }
    principal.check(name).map_err(deny)?;

    let executor = engine.executor();
    match name {
        "act:push" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().push(&pid, &tid, options).await)
        }
        "act:remove" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().remove(&pid, &tid, options).await)
        }
        "act:submit" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().submit(&pid, &tid, options).await)
        }
        "act:complete" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().complete(&pid, &tid, options).await)
        }
        "act:abort" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().abort(&pid, &tid, options).await)
        }
        "act:cancel" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().cancel(&pid, &tid, options).await)
        }
        "act:back" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().back(&pid, &tid, options).await)
        }
        "act:skip" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().skip(&pid, &tid, options).await)
        }
        "act:error" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.act().fail(&pid, &tid, options).await)
        }
        // model
        "model:ls" => {
            let query = options
                .get::<crate::query::Query>("query")
                .unwrap_or_else(|| crate::query::Query::new().limit(100));
            value(executor.model().list(&query).await)
        }
        "model:rm" => {
            let id = pop(&mut options, "id")?;
            value(executor.model().rm(&id).await)
        }
        "model:get" => {
            let id = pop(&mut options, "id")?;
            let fmt = options.get::<String>("fmt").unwrap_or("text".to_string());
            value(executor.model().get(&id, &fmt).await)
        }
        "model:deploy" => {
            let model_text = options
                .get::<String>("model")
                .ok_or_else(|| Error::Invalid("model is required".to_string()))?;
            let mut model =
                Workflow::from_yml(&model_text).map_err(|e| Error::Invalid(e.to_string()))?;
            if let Some(mid) = options.get::<String>("mid") {
                model.set_id(&mid);
            }
            let view = options.get::<JsonValue>("view");
            value(executor.model().deploy(&model, view.as_ref()).await)
        }
        // package
        "pack:ls" => {
            let query = options
                .get::<crate::query::Query>("query")
                .unwrap_or_else(|| crate::query::Query::new().limit(100));
            value(executor.pack().list(&query).await)
        }
        "pack:get" => {
            let id = pop(&mut options, "id")?;
            value(executor.pack().get(&id).await)
        }
        "pack:publish" => {
            let id = options
                .get::<String>("id")
                .ok_or_else(|| Error::Invalid("package 'id' is required".to_string()))?;
            let pack_name = options.get::<String>("name").unwrap_or_default();
            let desc = options.get::<String>("desc").unwrap_or_default();
            let icon = options.get::<String>("icon").unwrap_or_default();
            let doc = options.get::<String>("doc").unwrap_or_default();
            let version = options.get::<String>("version").unwrap_or_default();
            let schema = options
                .get::<serde_json::Value>("schema")
                .unwrap_or_default();
            let pack_options = options
                .get::<Option<serde_json::Value>>("options")
                .unwrap_or_default();
            let run_as = options.get::<String>("run_as").unwrap_or_default();
            let resources = options
                .get::<Vec<crate::ActResource>>("resources")
                .unwrap_or_default();
            let catalog = options.get::<String>("catalog").unwrap_or_default();
            let pack = crate::data::Package {
                id,
                name: pack_name,
                desc,
                icon,
                doc,
                version,
                schema: schema.to_string(),
                options: pack_options.map(|v| v.to_string()),
                run_as: std::str::FromStr::from_str(&run_as)
                    .map_err(|_err| Error::Invalid("package 'run_as' is invalid".to_string()))?,
                resources: serde_json::to_string(&resources)
                    .map_err(|e| Error::Internal(e.to_string()))?,
                catalog: std::str::FromStr::from_str(&catalog)
                    .map_err(|_err| Error::Invalid("package 'catalog' is invalid".to_string()))?,
                ..Default::default()
            };
            value(executor.pack().publish(&pack).await)
        }
        "pack:rm" => {
            let id = pop(&mut options, "id")?;
            value(executor.pack().rm(&id).await)
        }
        // proc
        "proc:start" => {
            let id = pop(&mut options, "id")?;
            // The caller's authority is sealed into the process (see
            // `crate::acl`) and re-checked at every seal: which snapshot
            // scopes the run may read, and where its filesystem access is
            // confined. It carries the workdir root too, so the process id
            // turns it into the run's own directory at start — nothing about
            // the directory comes from the request.
            options.set(consts::PROC_OWNER, principal.scope_policy());
            value(executor.proc().start(&id, options).await)
        }
        "proc:start_from_model" => {
            let fmt = pop(&mut options, "fmt")?;
            let model = pop(&mut options, "model")?;
            options.set(consts::PROC_OWNER, principal.scope_policy());
            value(
                executor
                    .proc()
                    .start_from_model(&model, &fmt, options)
                    .await,
            )
        }
        "proc:ls" => {
            let query = options
                .get::<crate::query::Query>("query")
                .unwrap_or_else(|| crate::query::Query::new().limit(100));
            value(executor.proc().list(&query).await)
        }
        "proc:get" => {
            let pid = pop(&mut options, "pid")?;
            value(executor.proc().get(&pid).await)
        }
        // task
        "task:ls" => {
            let query = options
                .get::<crate::query::Query>("query")
                .unwrap_or_else(|| crate::query::Query::new().limit(100));
            value(executor.task().list(&query).await)
        }
        "task:get" => {
            let pid = pop(&mut options, "pid")?;
            let tid = pop(&mut options, "tid")?;
            value(executor.task().get(&pid, &tid).await)
        }
        // msg
        "msg:ls" => {
            let query = options
                .get::<crate::query::Query>("query")
                .unwrap_or_else(|| crate::query::Query::new().limit(100));
            value(executor.msg().list(&query).await)
        }
        "msg:get" => {
            let id = pop(&mut options, "id")?;
            value(executor.msg().get(&id).await)
        }
        "msg:ack" => {
            let id = pop(&mut options, "id")?;
            value(executor.msg().ack(&id).await)
        }
        "msg:redo" => match options.get::<String>("id") {
            // re-send one error delivery to its channel
            Some(id) => value(executor.msg().redeliver(&id).await),
            // re-send every error delivery
            None => value(executor.msg().redo().await),
        },
        "msg:clear" => {
            if let Some(id) = options.get::<String>("id") {
                // clear one error delivery
                value(executor.msg().clear_delivery(&id).await)
            } else {
                let pid = options.get::<String>("pid");
                value(executor.msg().clear(pid).await)
            }
        }
        "msg:rm" => {
            let id = pop(&mut options, "id")?;
            value(executor.msg().rm(&id).await)
        }
        "msg:unsub" => {
            let client_id = pop(&mut options, "client_id")?;
            value(executor.msg().unsub(&client_id).await)
        }
        // event
        "evt:ls" => {
            let query = options
                .get::<crate::query::Query>("query")
                .unwrap_or_else(|| crate::query::Query::new().limit(100));
            value(executor.evt().list(&query).await)
        }
        "evt:get" => {
            let id = pop(&mut options, "id")?;
            value(executor.evt().get(&id).await)
        }
        "evt:start" => {
            let id = pop(&mut options, "id")?;
            let params = options.get::<JsonValue>("params").unwrap_or_default();
            // Triggers reach a model's own start inputs, so they carry no
            // caller authority: a snapshot-backed model started by a trigger
            // keeps the reading authority of whoever deployed it.
            value(executor.evt().start(&id, &params).await)
        }
        // snapshot — the requested scope must belong to the subject
        "snap:upsert" => {
            let target = pop(&mut options, "name")?;
            let scope = options.get::<String>("scope").unwrap_or_default();
            principal.check_scope(&target, &scope).map_err(deny)?;
            let rev = options
                .get::<u64>("rev")
                .ok_or_else(|| Error::Invalid("rev is required".to_string()))?;
            let data = options
                .pop::<Vars>("data")
                .ok_or_else(|| Error::Invalid("data is required".to_string()))?;
            unit_ok(engine.snapshot().upsert(&target, &scope, rev, data))
        }
        "snap:remove" => {
            let target = pop(&mut options, "name")?;
            let scope = options.get::<String>("scope").unwrap_or_default();
            principal.check_scope(&target, &scope).map_err(deny)?;
            unit_ok(engine.snapshot().remove(&target, &scope))
        }
        "snap:get" => {
            let target = pop(&mut options, "name")?;
            let scope = options.get::<String>("scope").unwrap_or_default();
            principal.check_scope(&target, &scope).map_err(deny)?;
            match engine.snapshot().read(&target, &scope) {
                Some(entry) => {
                    let data = serde_json::to_value(entry.data)
                        .map_err(|e| Error::Internal(e.to_string()))?;
                    Ok(json!({
                        "scope": scope,
                        "rev": entry.rev,
                        "timestamp": entry.timestamp,
                        "data": data,
                    }))
                }
                None => Ok(JsonValue::Null),
            }
        }
        "snap:ls" => {
            let target = pop(&mut options, "name")?;
            // A listing answers only the scopes this subject owns — the
            // others are filtered out rather than failing the whole call, so
            // one tenant cannot enumerate the rest.
            let rows: Vec<JsonValue> = engine
                .snapshot()
                .list(&target)
                .into_iter()
                .filter(|(scope, _)| principal.check_scope(&target, scope).is_ok())
                .map(|(scope, entry)| {
                    let data = serde_json::to_value(entry.data)
                        .map_err(|e| Error::Internal(e.to_string()))?;
                    Ok(json!({
                        "scope": scope,
                        "rev": entry.rev,
                        "timestamp": entry.timestamp,
                        "data": data,
                    }))
                })
                .collect::<std::result::Result<_, Error>>()?;
            Ok(JsonValue::Array(rows))
        }
        _ => Err(Error::NotFound(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_upsert_remove_roundtrip() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let payload = Vars::new()
            .with("name", "profile")
            .with("scope", "u1")
            .with("rev", 7u64)
            .with("data", Vars::new().with("val", "x"));
        let ret = apply(&engine, "snap:upsert", payload).await.unwrap();
        assert_eq!(ret, json!(true));

        let entry = engine.snapshot().read("profile", "u1").unwrap();
        assert_eq!(entry.rev, 7);
        assert_eq!(entry.data.get::<String>("val").unwrap(), "x".to_string());

        let ret = apply(
            &engine,
            "snap:remove",
            Vars::new().with("name", "profile").with("scope", "u1"),
        )
        .await
        .unwrap();
        assert_eq!(ret, json!(true));
        assert!(engine.snapshot().read("profile", "u1").is_none());
    }

    #[tokio::test]
    async fn unknown_action_is_not_found() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let err = apply(&engine, "no:such", Vars::new()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
        assert_eq!(err.to_string(), "not found action 'no:such'");
    }

    #[tokio::test]
    async fn missing_payload_is_invalid() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let err = apply(&engine, "snap:upsert", Vars::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)));
        assert!(err.to_string().contains("name is required"));
    }

    #[tokio::test]
    async fn snapshot_query_roundtrip() {
        let engine = crate::Engine::builder().start().await.unwrap();
        for (scope, val) in [("u1", 1), ("u2", 2)] {
            let payload = Vars::new()
                .with("name", "profile")
                .with("scope", scope)
                .with("rev", val)
                .with("data", Vars::new().with("val", val));
            let ret = apply(&engine, "snap:upsert", payload).await.unwrap();
            assert_eq!(ret, json!(true));
        }

        let ret = apply(
            &engine,
            "snap:get",
            Vars::new().with("name", "profile").with("scope", "u1"),
        )
        .await
        .unwrap();
        assert_eq!(ret["scope"], "u1");
        assert_eq!(ret["rev"], 1);
        assert_eq!(ret["data"]["val"], 1);

        let ret = apply(
            &engine,
            "snap:get",
            Vars::new().with("name", "profile").with("scope", "nope"),
        )
        .await
        .unwrap();
        assert_eq!(ret, JsonValue::Null);

        let ret = apply(&engine, "snap:ls", Vars::new().with("name", "profile"))
            .await
            .unwrap();
        let rows = ret.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|r| r["scope"] == "u1" && r["data"]["val"] == 1)
        );
        assert!(
            rows.iter()
                .any(|r| r["scope"] == "u2" && r["data"]["val"] == 2)
        );
    }

    #[tokio::test]
    async fn snapshot_query_unknown_target() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let ret = apply(&engine, "snap:ls", Vars::new().with("name", "none"))
            .await
            .unwrap();
        assert_eq!(ret, JsonValue::Array(vec![]));
    }

    #[tokio::test]
    async fn snapshot_remove_unknown_target_is_internal() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let err = apply(
            &engine,
            "snap:remove",
            Vars::new().with("name", "none").with("scope", "u1"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Internal(_)), "got: {err}");
        assert!(err.to_string().contains("none"), "got: {err}");
    }

    // ---- access control ----

    const MULTI_TENANT: &str = r#"
        [acl]

        [[acl.role]]
        name = "u1"
        tokens = ["token-u1"]
        allow = ["model:deploy", "proc:start", "snap:get", "snap:ls", "snap:upsert"]
        snapshot = { secrets = ["u1"], profile = ["u1"] }

        [[acl.role]]
        name = "u2"
        tokens = ["token-u2"]
        allow = ["model:deploy", "proc:start", "snap:get", "snap:ls", "snap:upsert"]
        snapshot = { secrets = ["u2"], profile = ["u2"] }
    "#;

    /// An engine whose `[acl]` section is the given toml text.
    async fn acl_engine(text: &str) -> crate::Engine {
        let config = crate::Config {
            data: Default::default(),
            table: toml::from_str::<toml::Table>(text).unwrap(),
        };
        crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_enabled_acl_refuses_the_anonymous_caller() {
        let engine = acl_engine(MULTI_TENANT).await;

        // `apply` is the anonymous in-process entry: no token, no access.
        let err = apply(&engine, "model:ls", Vars::new()).await.unwrap_err();
        assert!(matches!(err, Error::Unauthenticated(_)), "got: {err}");

        // A token that selects no role is refused the same way.
        let err = engine.acl().authenticate(Some("bogus")).unwrap_err();
        assert!(matches!(err, crate::AclError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn a_role_runs_only_the_actions_it_allows() {
        let engine = acl_engine(MULTI_TENANT).await;
        let principal = engine.acl().authenticate(Some("token-u1")).unwrap();

        apply_as(&engine, &principal, "model:ls", Vars::new())
            .await
            .unwrap_err();

        // whoami is implicitly allowed, even though `model:ls` is not.
        let who = apply_as(&engine, &principal, crate::acl::ACTION_WHOAMI, Vars::new())
            .await
            .unwrap();
        assert_eq!(who["subject"], "u1");
        assert_eq!(who["unrestricted"], false);
    }

    #[tokio::test]
    async fn a_snapshot_scope_belongs_to_one_subject_only() {
        let engine = acl_engine(MULTI_TENANT).await;
        let u1 = engine.acl().authenticate(Some("token-u1")).unwrap();
        let u2 = engine.acl().authenticate(Some("token-u2")).unwrap();

        // Each subject may seed and read its own scope.
        for (principal, scope, val) in [(&u1, "u1", 1), (&u2, "u2", 2)] {
            let payload = Vars::new()
                .with("name", "profile")
                .with("scope", scope)
                .with("rev", 1u64)
                .with("data", Vars::new().with("val", val));
            assert_eq!(
                apply_as(&engine, principal, "snap:upsert", payload)
                    .await
                    .unwrap(),
                json!(true)
            );
        }

        // ...and neither may touch the other's, in either direction.
        let payload = Vars::new()
            .with("name", "profile")
            .with("scope", "u2")
            .with("rev", 9u64)
            .with("data", Vars::new().with("val", 9));
        let err = apply_as(&engine, &u1, "snap:upsert", payload)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Denied(_)), "got: {err}");

        let err = apply_as(
            &engine,
            &u1,
            "snap:get",
            Vars::new().with("name", "profile").with("scope", "u2"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Denied(_)), "got: {err}");

        // A listing answers own scopes only — no enumeration of the others.
        let rows = apply_as(&engine, &u1, "snap:ls", Vars::new().with("name", "profile"))
            .await
            .unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["scope"], "u1");
        assert_eq!(rows[0]["data"]["val"], 1);
    }

    /// The decisive case: a workflow may not read another subject's sealed
    /// data just because it was started with that subject's `uid`. The
    /// authority is the caller's, sealed into the process at start.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn a_run_cannot_seal_another_subjects_scope() {
        let engine = acl_engine(MULTI_TENANT).await;
        engine
            .add_snapshot(
                "secrets",
                crate::SnapshotOptions {
                    scope: vec!["uid".to_string()],
                    ..crate::SnapshotOptions::per_proc()
                },
            )
            .unwrap();

        let u1 = engine.acl().authenticate(Some("token-u1")).unwrap();
        let u2 = engine.acl().authenticate(Some("token-u2")).unwrap();

        // u2's secret exists, and only u2 may read or write it.
        apply_as(
            &engine,
            &u2,
            "snap:upsert",
            Vars::new()
                .with("name", "secrets")
                .with("scope", "u2")
                .with("rev", 1u64)
                .with("data", Vars::new().with("TOKEN", "u2-secret")),
        )
        .await
        .unwrap();

        let workflow = crate::Workflow::from_yml(
            r#"
            id: acl_seal
            ver: 0.1.0
            exposes:
              - name: leaked
            steps:
              - name: read the secret
                uses: acts.transform.code
                params: |
                  return { leaked: secrets.TOKEN };
            "#,
        )
        .unwrap();
        apply_as(
            &engine,
            &u1,
            "model:deploy",
            Vars::new().with("model", workflow.to_yml().unwrap()),
        )
        .await
        .unwrap();

        // u1 starting a run under u2's uid must not reach u2's value: the
        // process carries u1's authority, so the seal is refused.
        let sig = engine.signal(String::new());
        let s = sig.clone();
        engine.channel().on_error(move |e| {
            let s = s.clone();
            async move {
                let err = e
                    .inputs
                    .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
                    .unwrap_or_default();
                s.update(|data| data.clone_from(&err));
                s.close();
            }
        });
        apply_as(
            &engine,
            &u1,
            "proc:start",
            Vars::new().with("id", "acl_seal").with("uid", "u2"),
        )
        .await
        .unwrap();
        let err = sig.recv().await;
        assert!(err.contains("not owned by subject 'u1'"), "got: {err}");

        // The same run under its own uid seals its own value.
        engine
            .snapshot()
            .upsert("secrets", "u1", 1, Vars::new().with("TOKEN", "u1-secret"))
            .unwrap();
        let sig = engine.signal(String::new());
        let s = sig.clone();
        engine.channel().on_complete(move |e| {
            let s = s.clone();
            async move {
                s.update(|data| data.clone_from(&e.pid));
                s.close();
            }
        });
        let pid = apply_as(
            &engine,
            &u1,
            "proc:start",
            Vars::new()
                .with("id", "acl_seal")
                .with("pid", "acl_seal_owned")
                .with("uid", "u1"),
        )
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
        assert_eq!(sig.recv().await, pid);

        // The run sealed its own subject's value, and nothing else.
        let proc = engine.runtime().proc(&pid).await.unwrap().unwrap();
        assert_eq!(
            proc.root()
                .unwrap()
                .sealed("secrets")
                .unwrap()
                .get::<String>("TOKEN")
                .unwrap(),
            "u1-secret"
        );
    }

    /// A run's directory is its own: `<root>/<pid>`, sealed into the process
    /// and reachable by a package through `Context::workdir`.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn a_run_is_confined_to_its_own_workdir() {
        let root = std::env::temp_dir().join(format!("acts_workdir_{}", crate::utils::longid()));
        let engine = acl_engine(&format!(
            r#"
            [acl]
            workdir = '{}'

            [[acl.role]]
            name = "u1"
            tokens = ["token-u1"]
            allow = ["model:deploy", "proc:start"]
            "#,
            root.display()
        ))
        .await;

        // The root itself is not created up front: a policy may name one that
        // does not exist yet, and the first run materializes its own directory.
        assert!(!root.exists());

        let workflow = crate::Workflow::from_yml(
            r#"
            id: workdir_run
            ver: 0.1.0
            steps:
              - name: finish
                uses: acts.transform.code
                params: |
                  return { dir: $env.WORK_DIR };
            "#,
        )
        .unwrap();
        let u1 = engine.acl().authenticate(Some("token-u1")).unwrap();
        assert_eq!(u1.workdir_root(), Some(root.as_path()));

        // the run's completion, so its script's reading below is in hand
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });

        apply_as(
            &engine,
            &u1,
            "model:deploy",
            Vars::new().with("model", workflow.to_yml().unwrap()),
        )
        .await
        .unwrap();

        let pid = apply_as(
            &engine,
            &u1,
            "proc:start",
            Vars::new().with("id", "workdir_run").with("pid", "run1"),
        )
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
        assert_eq!(pid, "run1");

        // The run got `<root>/<pid>`, and the workflow never saw the key.
        let dir = root.join("run1");
        assert!(dir.is_dir(), "workdir {} was not created", dir.display());
        let proc = engine.runtime().proc(&pid).await.unwrap().unwrap();
        assert_eq!(proc.workdir(), Some(dir.clone()));
        assert!(proc.inputs().get::<String>(consts::PROC_WORKDIR).is_none());

        // ...and the run's own script finds the directory by name, the public
        // alias of the private key the workflow never saw
        sig.recv().await;
        assert_eq!(
            proc.task_by_uses(crate::utils::test::USES_CODE)
                .first()
                .unwrap()
                .outputs()
                .get::<String>("dir")
                .unwrap(),
            dir.display().to_string()
        );
        // The directory lives as long as the run's durable rows: once the run
        // finished and its deliveries settled, the sweeper that removes the
        // rows takes the workdir with them — a long-lived engine does not
        // accumulate one directory per historical process.
        for _ in 0..150 {
            if !dir.exists() {
                break;
            }
            let _ = engine.runtime().cache().sweep_removable().await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            !dir.exists(),
            "the workdir {} must be removed with the process's rows",
            dir.display()
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A pid is not a free-form string once it names a directory: a value that
    /// would place the run outside its root (or leave the filesystem's own
    /// segment vocabulary) is refused instead of confined.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn a_workdir_refuses_a_pid_that_is_not_one_directory() {
        let root = std::env::temp_dir().join(format!("acts_workdir_{}", crate::utils::longid()));
        let engine = acl_engine(&format!(
            r#"
            [acl]
            workdir = '{}'

            [[acl.role]]
            name = "u1"
            tokens = ["token-u1"]
            allow = ["proc:start_from_model"]
            "#,
            root.display()
        ))
        .await;
        let u1 = engine.acl().authenticate(Some("token-u1")).unwrap();
        let model = Vars::new()
            .with("model", "id: w\nver: 0.1.0\nsteps:\n  - name: s\n")
            .with("fmt", "yml");

        for pid in ["..", ".", "a/b", "a\\b", "a:b"] {
            let err = apply_as(
                &engine,
                &u1,
                "proc:start_from_model",
                model.clone().with("pid", pid),
            )
            .await
            .expect_err(pid);
            assert!(
                err.to_string().contains("cannot be used as a workdir name"),
                "pid {pid:?} must be refused, got: {err}"
            );
        }

        // Nothing escaped: the root has no entries, not even `..`
        // materialized somewhere else.
        assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
