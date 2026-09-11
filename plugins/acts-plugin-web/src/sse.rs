use crate::objects::{AppError, RespData};
use acts::{Channel, ChannelOptions, Engine, Message, Vars};
use axum::{
    Json,
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

pub async fn sse(
    State(state): State<Arc<Engine>>,
    Query(query): Query<MessageQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, mut rx) = mpsc::channel::<Message>(100);

    let chan = state.channel_with_options(&ChannelOptions {
        id: format!("acts-flow-client-{}", query.id),
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
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn ack(
    State(state): State<Arc<Engine>>,
    Json(ack): Json<MessageAck>,
) -> Result<impl IntoResponse, AppError> {
    state.executor().msg().ack(&ack.id).await?;
    Ok(RespData::ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts::Workflow;
    use acts::query::Query as StoreQuery;
    use std::sync::Mutex;
    use std::time::Duration;

    async fn run_irq_workflow(engine: &Engine, key: &str) {
        let model = Workflow::new()
            .with_id(&format!("sse-leak-{key}"))
            .with_step(|step| {
                step.with_id("step1")
                    .with_uses("acts.core.irq", Vars::new().with("key", "leak-test"))
            });
        engine
            .executor()
            .model()
            .deploy(&model, None)
            .await
            .unwrap();
        engine
            .executor()
            .proc()
            .start(&model.id, Vars::new())
            .await
            .unwrap();
    }

    async fn stored_message_count(engine: &Engine) -> usize {
        engine
            .executor()
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
        let engine = Engine::builder().start().await.unwrap();

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
        let response = sse(State(Arc::new(engine.clone())), Query(query)).await;

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
}
