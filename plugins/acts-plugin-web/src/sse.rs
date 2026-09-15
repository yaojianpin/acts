use crate::objects::{AppError, RespData};
use acts::{Channel, ChannelOptions, Engine, Message, Principal, Vars};
use axum::{
    Extension, Json,
    extract::{Query, State},
    response::{
        IntoResponse, Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::stream::Stream;
use serde::Deserialize;
use std::{convert::Infallible, sync::Arc};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Deserialize)]
pub struct MessageQuery {
    pub id: String,
    pub r#type: Option<String>,
    pub uses: Option<String>,
    pub state: Option<String>,
    #[allow(dead_code)]
    pub key: Option<String>,
    #[serde(default)]
    pub options: Vars,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageAck {
    pub id: String,
}

/// Deregisters the channel handler when the SSE response stream is dropped
/// (client disconnected or the stream ended). Without this, every finished
/// SSE connection would leak a handler into the engine emitter: the map grows
/// unboundedly and every future message pays glob matching plus ack-delivery
/// store writes for dead channels.
struct CloseChannelOnDrop {
    chan: Arc<Channel>,
}

impl Drop for CloseChannelOnDrop {
    fn drop(&mut self) {
        self.chan.close();
    }
}

/// Subscribe to workflow events over SSE.
///
/// The subscription is an action like any other: the route middleware
/// authenticates the caller, and this handler authorizes it as `msg:sub`, so
/// a role without that grant is answered `403` instead of a stream. The key
/// the action answers with is the channel the stream registers under — the
/// transport never composes it itself, so the checked path and the occupied
/// key cannot drift apart. The `type`/`state`/`uses`/`options` filter stays
/// self-declared, exactly like a gRPC `on_message` subscription.
pub async fn sse(
    State(state): State<Arc<Engine>>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<MessageQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let chan_id = acts::actions::apply_as(
        &state,
        &principal,
        acts::ACTION_SUBSCRIBE,
        Vars::new().with("client_id", format!("acts-flow-client-{}", query.id)),
    )
    .await?
    .as_str()
    .unwrap_or_default()
    .to_string();

    let (tx, mut rx) = mpsc::channel::<Message>(100);

    let chan = state.channel_with_options(&ChannelOptions {
        id: chan_id,
        ack: true,
        r#type: query.r#type.unwrap_or("*".to_string()),
        state: query.state.unwrap_or("*".to_string()),
        uses: query.uses.unwrap_or("*".to_string()),
        options: query.options,
    });
    chan.on_message(move |e| {
        let tx = tx.clone();
        async move {
            let msg = e.inner().clone();
            tokio::spawn(async move { tx.send(msg).await });
        }
    });

    // The guard lives inside the stream generator: axum drops the response
    // body when the client disconnects, which closes the channel and
    // deregisters the handler. When the handler is gone its sender is
    // dropped too, so the stream below ends instead of spinning on a
    // closed queue.
    let guard = CloseChannelOnDrop { chan: chan.clone() };
    let stream = async_stream::stream! {
        let _guard = guard;
        // when all sender clones are gone (channel deregistered), the
        // stream ends instead of spinning on a closed queue
        while let Some(data) = rx.recv().await {
            let message = serde_json::to_string(&data).unwrap_or_default();
            yield Ok(Event::default().data(message))
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

pub async fn ack(
    State(state): State<Arc<Engine>>,
    Extension(principal): Extension<Principal>,
    Json(ack): Json<MessageAck>,
) -> Result<impl IntoResponse, AppError> {
    let value = acts::actions::apply_as(
        &state,
        &principal,
        "msg:ack",
        Vars::new().with("id", ack.id),
    )
    .await?;
    Ok(RespData::ok(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts::query::Query as StoreQuery;
    use acts::{Workflow, actions};
    use axum::body::Bytes;
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Two tenants, both allowed to start runs, subscribe and ack.
    const SUB_ACL: &str = r#"
[acl]

[[acl.role]]
name = "u1"
tokens = ["token-u1"]
allow = ["model:deploy", "proc:start", "msg:ack", "msg:sub"]

[[acl.role]]
name = "u2"
tokens = ["token-u2"]
allow = ["model:deploy", "proc:start", "msg:ack", "msg:sub"]
"#;

    async fn engine_with_acl(text: &str) -> Engine {
        let table: toml::Table = toml::from_str(text).unwrap();
        let config = acts::Config {
            data: Default::default(),
            table,
        };
        Engine::builder().set_config(&config).start().await.unwrap()
    }

    /// Open the SSE body for one subscriber and answer its event stream.
    async fn subscribe(
        engine: &Engine,
        principal: &Principal,
        id: &str,
    ) -> impl Stream<Item = Result<Bytes, axum::Error>> + Unpin {
        let query = MessageQuery {
            id: id.to_string(),
            r#type: None,
            uses: None,
            state: None,
            key: None,
            options: Vars::new(),
        };
        let response = sse(
            State(Arc::new(engine.clone())),
            Extension(principal.clone()),
            Query(query),
        )
        .await
        .into_response();
        response.into_body().into_data_stream()
    }

    /// Read the SSE frames until the stream goes quiet for `idle_ms`, decoding
    /// each `data:` payload back into an `acts::Message`.
    async fn drain<S>(stream: &mut S, idle_ms: u64) -> Vec<Message>
    where
        S: Stream<Item = Result<Bytes, axum::Error>> + Unpin,
    {
        let mut out = Vec::new();
        let mut pending = String::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(idle_ms), stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    pending.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(end) = pending.find("\n\n") {
                        let frame = pending[..end].to_string();
                        pending.drain(..end + 2);
                        for line in frame.lines() {
                            // keep-alive comments carry no payload
                            let Some(data) = line.strip_prefix("data: ") else {
                                continue;
                            };
                            out.push(serde_json::from_str::<Message>(data).unwrap());
                        }
                    }
                }
                Ok(Some(Err(err))) => panic!("SSE stream failed: {err}"),
                Ok(None) => break,
                Err(_) => break,
            }
        }
        out
    }

    /// Start the deployed `mid` as the principal's own run and answer its pid.
    async fn start_owned(engine: &Engine, principal: &Principal, mid: &str) -> String {
        actions::apply_as(engine, principal, "proc:start", Vars::new().with("id", mid))
            .await
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn run_irq_workflow(engine: &Engine, key: &str) {
        let model = Workflow::new()
            .with_id(&format!("sse-leak-{key}"))
            .with_step(|step| {
                step.with_id("step1")
                    .with_uses("acts.core.irq", Vars::new().with("key", "leak-test"))
            });
        engine
            .executor(&acts::Principal::unrestricted())
            .model()
            .deploy(&model, None)
            .await
            .unwrap();
        engine
            .executor(&acts::Principal::unrestricted())
            .proc()
            .start(&model.id, Vars::new())
            .await
            .unwrap();
    }

    async fn stored_message_count(engine: &Engine) -> usize {
        engine
            .executor(&acts::Principal::unrestricted())
            .msg()
            .list(&StoreQuery::new().offset(0).limit(1000))
            .await
            .unwrap()
            .count
    }

    async fn wait_until(mut cond: impl FnMut() -> bool, label: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {label}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A finished SSE connection must deregister its channel handler,
    /// otherwise the dead handler keeps matching every future message and
    /// stores ack deliveries (message rows are only written by ack-channel
    /// deliveries, so the stored message count is the leak detector).
    #[tokio::test(flavor = "multi_thread")]
    async fn sse_stream_drop_deregisters_channel_handler() {
        // the SSE transport surface, not the policy: open the engine explicitly
        // (an unconfigured one is anonymous and read-only)
        let engine = engine_with_acl("[acl]\nenabled = false\n").await;

        // spy channel: counts every dispatched workflow message without
        // storing anything (ack = false)
        let received = Arc::new(Mutex::new(0usize));
        let spy = engine.channel_with_options(&ChannelOptions {
            ack: false,
            ..Default::default()
        });
        let spy_received = received.clone();
        spy.on_message(move |_e| {
            let received = spy_received.clone();
            async move {
                *received.lock().unwrap() += 1;
            }
        });

        let query = MessageQuery {
            id: "sse-leak-test".to_string(),
            r#type: None,
            uses: None,
            state: None,
            key: None,
            options: Vars::new(),
        };
        let response = sse(
            State(Arc::new(engine.clone())),
            Extension(engine.anonymous()),
            Query(query),
        )
        .await;

        // positive control: while the stream is alive, its ack channel
        // stores one message row per workflow message
        run_irq_workflow(&engine, "alive").await;
        wait_until(
            || *received.lock().unwrap() >= 3,
            "workflow messages to be dispatched",
        )
        .await;
        let alive_rows = stored_message_count(&engine).await;
        assert!(alive_rows >= 1, "live channel should store deliveries");

        // simulate the client disconnecting: axum drops the response body
        drop(response);

        // new messages must not be stored for the dead channel anymore
        run_irq_workflow(&engine, "dropped").await;
        wait_until(
            || *received.lock().unwrap() >= 6,
            "messages of the second workflow to be dispatched",
        )
        .await;
        assert_eq!(
            stored_message_count(&engine).await,
            alive_rows,
            "dropped SSE channel must not store further deliveries"
        );

        engine.close().await;
    }

    /// The `id` of the query is not the channel key on its own: the caller's
    /// subject namespaces it, so two subscribers naming the same id coexist
    /// instead of one replacing the other's handler. Both see every process —
    /// delivery follows the grants and the channel filters, not the run's
    /// starter.
    #[tokio::test(flavor = "multi_thread")]
    async fn sse_subscriptions_are_namespaced_by_subject() {
        let engine = engine_with_acl(SUB_ACL).await;
        let u1 = engine.acl().authenticate(Some("token-u1")).unwrap();
        let u2 = engine.acl().authenticate(Some("token-u2")).unwrap();

        let model = Workflow::new().with_id("sse-scope").with_step(|step| {
            step.with_id("step1")
                .with_uses("acts.core.irq", Vars::new().with("key", "scope-test"))
        });
        for principal in [&u1, &u2] {
            actions::apply_as(
                &engine,
                principal,
                "model:deploy",
                Vars::new().with("model", model.to_yml().unwrap()),
            )
            .await
            .unwrap();
        }

        // the same client id on both sides, a different token each
        let mut u1_stream = subscribe(&engine, &u1, "shared-client").await;
        let mut u2_stream = subscribe(&engine, &u2, "shared-client").await;

        let u1_pid = start_owned(&engine, &u1, "sse-scope").await;
        let u2_pid = start_owned(&engine, &u2, "sse-scope").await;

        let seen_u1 = drain(&mut u1_stream, 1_000).await;
        let seen_u2 = drain(&mut u2_stream, 1_000).await;

        // Both registrations are live — neither handler replaced the other —
        // and each stream carried both runs.
        assert!(!seen_u1.is_empty(), "u1's subscription received nothing");
        assert!(!seen_u2.is_empty(), "u2's subscription received nothing");
        for (subject, seen) in [("u1", &seen_u1), ("u2", &seen_u2)] {
            assert!(
                seen.iter().any(|m| m.pid == u1_pid),
                "{subject}'s stream never saw u1's run"
            );
            assert!(
                seen.iter().any(|m| m.pid == u2_pid),
                "{subject}'s stream never saw u2's run"
            );
        }

        engine.close().await;
    }

    /// A subscription is an action: a role without `msg:sub` gets a `403`
    /// instead of a stream, and one with it is granted a channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn sse_subscription_needs_the_grant() {
        let engine = engine_with_acl(
            r#"
[acl]
[[acl.role]]
name = "reader"
tokens = ["reader-token"]
allow = ["msg:ls"]
[[acl.role]]
name = "listener"
tokens = ["listener-token"]
allow = ["msg:sub"]
"#,
        )
        .await;
        let reader = engine.acl().authenticate(Some("reader-token")).unwrap();

        let err = sse(
            State(Arc::new(engine.clone())),
            Extension(reader),
            Query(MessageQuery {
                id: "reader-client".to_string(),
                r#type: None,
                uses: None,
                state: None,
                key: None,
                options: Vars::new(),
            }),
        )
        .await
        .expect_err("a role without msg:sub must not open a stream");
        assert_eq!(
            err.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );

        let granted = engine.acl().authenticate(Some("listener-token")).unwrap();
        let response = sse(
            State(Arc::new(engine.clone())),
            Extension(granted),
            Query(MessageQuery {
                id: "granted-client".to_string(),
                r#type: None,
                uses: None,
                state: None,
                key: None,
                options: Vars::new(),
            }),
        )
        .await
        .expect("msg:sub opens the stream");
        drop(response);

        engine.close().await;
    }
}
