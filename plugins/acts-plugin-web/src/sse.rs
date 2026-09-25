use crate::HttpConfig;
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
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::warn;

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

    let queue_size = state
        .config()
        .get::<HttpConfig>("web")
        .unwrap_or_default()
        .queue_size();
    let (tx, mut rx) = mpsc::channel::<Message>(queue_size);

    let chan = state.channel_with_options(&ChannelOptions {
        id: chan_id.clone(),
        ack: true,
        r#type: query.r#type.unwrap_or("*".to_string()),
        state: query.state.unwrap_or("*".to_string()),
        uses: query.uses.unwrap_or("*".to_string()),
        options: query.options,
    });
    // The handler hands a message to the stream and never waits for the client:
    // it writes into the bounded queue, and a message that does not fit means
    // the client stopped reading — the subscription is closed there and then.
    // Spawning a task per message to await room instead made the queue no bound
    // at all: every message past it left a task (and the message it held) alive
    // until the client read, which a client that never reads never does.
    //
    // A client disconnected this way loses nothing the engine still owes the
    // channel: a delivery that was handed over but not acked, of a process that
    // has not settled, is re-sent by the retry timer to the channel the client
    // registers again (the same client id composes the same key). As after any
    // disconnect, a channel only receives messages emitted while it is
    // registered, and a settled process's deliveries settle with it.
    //
    // The channel handle is weak because the handler is owned by the engine's
    // emitter, which the channel's own runtime owns: a strong reference would
    // keep the runtime alive from inside itself.
    let chan_ref = Arc::downgrade(&chan);
    // logged on the overflow path only; shared so a delivery does not clone a
    // channel key it never prints
    let chan_name = Arc::new(chan_id);
    chan.on_message(move |e| {
        let tx = tx.clone();
        let chan = chan_ref.clone();
        let chan_id = chan_name.clone();
        async move {
            match tx.try_send(e.inner().clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    warn!(chan = %chan_id, "SSE subscriber is not reading; closing the subscription");
                    if let Some(chan) = chan.upgrade() {
                        chan.close();
                    }
                }
                // the stream (and so the receiver) is already gone
                Err(TrySendError::Closed(_)) => {}
            }
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
    use acts_acl::AclUsers;
    use axum::body::Bytes;
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// The grants both tenants have: deploy and start runs, subscribe and ack.
    const SUB_ALLOW: &[&str] = &["model:deploy", "proc:start", "msg:ack", "msg:sub"];

    /// An engine with access control on — the default — and one user per
    /// `(name, password, allow)` entry. Credentials live in the store, not in
    /// the config file, so a principal carries the session token `login`
    /// answers. The store-backed registry is a separate crate: without it a
    /// bare engine refuses every `acl:*` action, `acl:login` and `set_user`
    /// included.
    async fn engine_with_users(users: &[(&str, &str, &[&str])]) -> Engine {
        let engine = Engine::builder().with_user_acl().start().await.unwrap();
        for (name, password, allow) in users {
            engine
                .acl()
                .set_user(&acts::UserSpec {
                    name: (*name).to_string(),
                    add_passwords: vec![(*password).to_string()],
                    allow: Some(allow.iter().map(|a| (*a).to_string()).collect()),
                    // both tenants deploy and start a model, so they own the
                    // `*` resource — the model's own `rn` must still be set
                    patterns: Some(vec!["*".to_string()]),
                    ..Default::default()
                })
                .await
                .unwrap();
        }
        engine
    }

    /// Log `user` in and answer the session token its requests carry.
    async fn login(engine: &Engine, user: &str, password: &str) -> String {
        engine.acl().login(user, password).await.unwrap().token
    }

    /// An unrestricted engine (`disable_acl`) with a two-message subscription
    /// queue and its retry tick pinned to a second: the redelivery case needs
    /// a queue one run can fill and a re-sent delivery rather than the
    /// 15-second default.
    async fn engine_for_redelivery() -> Engine {
        let table: toml::Table = toml::from_str("[web]\nqueue_size = 2\n").unwrap();
        let mut config = acts::Config {
            data: Default::default(),
            table,
        };
        config.data.tick_interval_secs = Some(1);
        Engine::builder()
            .set_config(&config)
            .disable_acl()
            .start()
            .await
            .unwrap()
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

    async fn wait_until(mut cond: impl AsyncFnMut() -> bool, label: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !cond().await {
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
        // the SSE transport surface, not the policy: these cases need an
        // unrestricted caller, so the engine runs with `disable_acl`
        let engine = Engine::builder().disable_acl().start().await.unwrap();

        // spy channel: counts every dispatched workflow message without
        // storing anything (ack = false)
        let received = Arc::new(AtomicUsize::new(0));
        let spy = engine.channel_with_options(&ChannelOptions {
            ack: false,
            ..Default::default()
        });
        let spy_received = received.clone();
        spy.on_message(move |_e| {
            let received = spy_received.clone();
            async move {
                received.fetch_add(1, Ordering::Relaxed);
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
        // wait for the stored rows, not just the dispatch: the dispatch
        // counter says nothing about this channel's own handler futures, and
        // a row one of them writes after `alive_rows` is sampled would fail
        // the leak check below
        wait_until(
            || async { stored_message_count(&engine).await >= 3 },
            "the live channel to store the workflow's deliveries",
        )
        .await;
        let alive_rows = stored_message_count(&engine).await;
        assert!(alive_rows >= 1, "live channel should store deliveries");

        // simulate the client disconnecting: axum drops the response body
        drop(response);

        // new messages must not be stored for the dead channel anymore
        run_irq_workflow(&engine, "dropped").await;
        wait_until(
            || async { received.load(Ordering::Relaxed) >= 6 },
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
        let engine =
            engine_with_users(&[("u1", "u1-pass", SUB_ALLOW), ("u2", "u2-pass", SUB_ALLOW)]).await;
        let u1_token = login(&engine, "u1", "u1-pass").await;
        let u2_token = login(&engine, "u2", "u2-pass").await;
        let u1 = engine.acl().authenticate(Some(&u1_token)).unwrap();
        let u2 = engine.acl().authenticate(Some(&u2_token)).unwrap();

        // the model names a resource both users own (`patterns = ["*"]`): a
        // model without an `rn` may only be deployed by an unrestricted user
        let model = Workflow::new()
            .with_id("sse-scope")
            .with_rn("sse:scope")
            .with_step(|step| {
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

        // the same client id on both sides, a different session each
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

    /// A subscription is an action: a user without `msg:sub` gets a `403`
    /// instead of a stream, and one with it is granted a channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn sse_subscription_needs_the_grant() {
        let engine = engine_with_users(&[
            ("reader", "reader-pass", &["msg:ls"]),
            ("listener", "listener-pass", &["msg:sub"]),
        ])
        .await;
        let reader_token = login(&engine, "reader", "reader-pass").await;
        let reader = engine.acl().authenticate(Some(&reader_token)).unwrap();

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
        .expect_err("a user without msg:sub must not open a stream");
        assert_eq!(
            err.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );

        let listener_token = login(&engine, "listener", "listener-pass").await;
        let granted = engine.acl().authenticate(Some(&listener_token)).unwrap();
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

    /// A subscriber that stops reading must be disconnected, not served
    /// forever: the handler writes into a bounded queue and never waits for
    /// room, so the queue is the whole backlog of a subscription — once it is
    /// full the subscription is closed instead of growing a waiting task per
    /// message. What the client was owed is not dropped with it: the deliveries
    /// it left unacked (of a process that has not settled) are re-sent to the
    /// channel it registers again.
    #[tokio::test(flavor = "multi_thread")]
    async fn sse_slow_subscriber_is_disconnected_instead_of_queueing() {
        // a two-message queue: one run already emits past the bound
        let engine = engine_for_redelivery().await;

        // spy channel: counts every dispatched workflow message without
        // storing anything (ack = false), so the emissions are observable
        // while the subscription under test is left unread
        let received = Arc::new(AtomicUsize::new(0));
        let spy = engine.channel_with_options(&ChannelOptions {
            ack: false,
            ..Default::default()
        });
        let spy_received = received.clone();
        spy.on_message(move |_e| {
            let received = spy_received.clone();
            async move {
                received.fetch_add(1, Ordering::Relaxed);
            }
        });

        // open a subscription and never read it: the response body is not
        // polled, so nothing drains its queue
        let query = MessageQuery {
            id: "slow-reader".to_string(),
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
        .await
        .expect("msg:sub opens the stream")
        .into_response();

        // two runs emit at least 6 messages, so the queue (2) is exceeded even
        // while the count lags the handler under test by the message in flight
        run_irq_workflow(&engine, "slow1").await;
        run_irq_workflow(&engine, "slow2").await;
        let seen = || received.load(Ordering::Relaxed);
        wait_until(
            || async { seen() >= 6 },
            "workflow messages to be dispatched",
        )
        .await;

        // the overflow closed the subscription: the stream flushes what was
        // queued and ends, instead of waiting for a reader that never came
        let mut stream = response.into_body().into_data_stream();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(err))) => panic!("SSE stream failed: {err}"),
                Ok(None) => break,
                Err(_) => panic!(
                    "a subscriber that stopped reading must be disconnected, not left queued"
                ),
            }
        }

        // and the channel is deregistered: later messages are not delivered to
        // (and not stored for) the closed subscription
        let rows = stored_message_count(&engine).await;
        run_irq_workflow(&engine, "after").await;
        wait_until(
            || async { seen() >= 9 },
            "messages of the last workflow to be dispatched",
        )
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            stored_message_count(&engine).await,
            rows,
            "a disconnected SSE channel must not store further deliveries"
        );

        // the disconnect loses nothing: the deliveries it left unacked are
        // re-sent by the retry timer to the channel this client occupies again
        // (the same client id composes the same channel key)
        let unrestricted = engine.anonymous();
        let mut second = subscribe(&engine, &unrestricted, "slow-reader").await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let mut redelivered = Vec::new();
        while redelivered.is_empty() && tokio::time::Instant::now() < deadline {
            redelivered = drain(&mut second, 1_000).await;
        }
        assert!(
            !redelivered.is_empty(),
            "a disconnected subscriber must receive its unacked deliveries again after resubscribing"
        );

        engine.close().await;
    }
}
