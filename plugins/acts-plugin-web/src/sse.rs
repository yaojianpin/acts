use crate::objects::{AppError, RespData};
use acts::{ChannelOptions, Engine, Message, Vars};
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
        let msg = e.inner().clone();
        let tx = tx.clone();
        tokio::spawn(async move { tx.send(msg).await });
    });

    let stream = async_stream::stream! {
        loop {
            if let Some(data) = rx.recv().await {
                let message = serde_json::to_string(&data).unwrap_or_default();
                yield Ok(Event::default().data(message))
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn ack(
    State(state): State<Arc<Engine>>,
    Json(ack): Json<MessageAck>,
) -> Result<impl IntoResponse, AppError> {
    state.executor().msg().ack(&ack.id)?;
    Ok(RespData::ok(()))
}
