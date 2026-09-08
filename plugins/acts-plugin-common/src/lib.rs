//! Shared message-action dispatch for acts transport plugins.
//!
//! Every inbound channel message is a `name` plus a `Vars` payload. This
//! crate maps the name to the matching engine operation (process/model/act/
//! msg/evt/snapshot) and returns the JSON-serialized result — the same wire
//! value the gRPC plugin used to produce. Transport plugins (`acts-plugin-grpc`,
//! `acts-plugin-nats`) call [`apply`] and marshal the value or the [`Error`]
//! into their own protocol, so every transport speaks one action set and the
//! table is maintained in a single place.
//!
//! Error kinds map to transport semantics:
//! - [`Error::NotFound`] — unknown action name (`not found`)
//! - [`Error::Invalid`] — malformed/missing payload fields (`invalid argument`)
//! - [`Error::Internal`] — engine/store failure (`internal error`)

use acts::{Engine, Vars, Workflow};
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(msg) => write!(f, "not found action '{msg}'"),
            Error::Invalid(msg) => f.write_str(msg),
            Error::Internal(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}

pub type Ret = std::result::Result<JsonValue, Error>;

/// Serialize an already-resolved engine result into its wire value.
fn value<T: serde::Serialize>(r: acts::Result<T>) -> Ret {
    serde_json::to_value(r.map_err(|e| Error::Internal(e.to_string()))?)
        .map_err(|e| Error::Internal(e.to_string()))
}

fn pop(options: &mut Vars, key: &str) -> std::result::Result<String, Error> {
    options
        .pop::<String>(key)
        .ok_or_else(|| Error::Invalid(format!("{key} is required")))
}

/// Apply a channel message action.
///
/// `name` selects the operation; `options` is the payload. On success the
/// returned value serializes exactly like the old gRPC `Message.data`.
pub async fn apply(engine: &Engine, name: &str, mut options: Vars) -> Ret {
    let executor = engine.executor();
    match name {
        // act
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
                .get::<acts::query::Query>("query")
                .unwrap_or_else(|| acts::query::Query::new().limit(100));
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
            value(executor.model().deploy(&model, None).await)
        }
        // package
        "pack:ls" => {
            let query = options
                .get::<acts::query::Query>("query")
                .unwrap_or_else(|| acts::query::Query::new().limit(100));
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
                .get::<Vec<acts::ActResource>>("resources")
                .unwrap_or_default();
            let catalog = options.get::<String>("catalog").unwrap_or_default();
            let pack = acts::data::Package {
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
            value(executor.proc().start(&id, options).await)
        }
        "proc:start_from_model" => {
            let fmt = pop(&mut options, "fmt")?;
            let model = pop(&mut options, "model")?;
            value(
                executor
                    .proc()
                    .start_from_model(&model, &fmt, options)
                    .await,
            )
        }
        "proc:ls" => {
            let query = options
                .get::<acts::query::Query>("query")
                .unwrap_or_else(|| acts::query::Query::new().limit(100));
            value(executor.proc().list(&query).await)
        }
        "proc:get" => {
            let pid = pop(&mut options, "pid")?;
            value(executor.proc().get(&pid).await)
        }
        // task
        "task:ls" => {
            let query = options
                .get::<acts::query::Query>("query")
                .unwrap_or_else(|| acts::query::Query::new().limit(100));
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
                .get::<acts::query::Query>("query")
                .unwrap_or_else(|| acts::query::Query::new().limit(100));
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
                .get::<acts::query::Query>("query")
                .unwrap_or_else(|| acts::query::Query::new().limit(100));
            value(executor.evt().list(&query).await)
        }
        "evt:get" => {
            let id = pop(&mut options, "id")?;
            value(executor.evt().get(&id).await)
        }
        "evt:start" => {
            let id = pop(&mut options, "id")?;
            let params = options.get::<JsonValue>("params").unwrap_or_default();
            value(executor.evt().start(&id, &params).await)
        }
        // snapshot
        "snap:upsert" => {
            let target = pop(&mut options, "name")?;
            let scope = options.get::<String>("scope").unwrap_or_default();
            let rev = options
                .get::<u64>("rev")
                .ok_or_else(|| Error::Invalid("rev is required".to_string()))?;
            let data = options
                .pop::<Vars>("data")
                .ok_or_else(|| Error::Invalid("data is required".to_string()))?;
            engine.snapshot().upsert(&target, &scope, rev, data);
            Ok(json!(true))
        }
        "snap:remove" => {
            let target = pop(&mut options, "name")?;
            let scope = options.get::<String>("scope").unwrap_or_default();
            engine.snapshot().remove(&target, &scope);
            Ok(json!(true))
        }
        "snap:get" => {
            let target = pop(&mut options, "name")?;
            let scope = options.get::<String>("scope").unwrap_or_default();
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
            let rows: Vec<JsonValue> = engine
                .snapshot()
                .list(&target)
                .into_iter()
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
        let engine = acts::Engine::new().start().await.unwrap();
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
        let engine = acts::Engine::new().start().await.unwrap();
        let err = apply(&engine, "no:such", Vars::new()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
        assert_eq!(err.to_string(), "not found action 'no:such'");
    }

    #[tokio::test]
    async fn missing_payload_is_invalid() {
        let engine = acts::Engine::new().start().await.unwrap();
        let err = apply(&engine, "snap:upsert", Vars::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)));
        assert!(err.to_string().contains("name is required"));
    }

    #[tokio::test]
    async fn snapshot_query_roundtrip() {
        let engine = acts::Engine::new().start().await.unwrap();
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
        let engine = acts::Engine::new().start().await.unwrap();
        let ret = apply(&engine, "snap:ls", Vars::new().with("name", "none"))
            .await
            .unwrap();
        assert_eq!(ret, JsonValue::Array(vec![]));
    }
}
