use serde::Deserialize;

/// Messages that may wait for one SSE subscriber when `[web].queue_size` is
/// not set.
pub const DEFAULT_QUEUE_SIZE: usize = 100;

/// Upper bound of `[web].queue_size`: a subscriber this far behind is not
/// going to catch up by being given more room.
const MAX_QUEUE_SIZE: usize = 65_536;

/// `[web]` — the HTTP transport, including the SSE message stream.
#[derive(Deserialize, Default)]
pub struct HttpConfig {
    pub port: Option<u32>,

    /// How many messages may wait for one SSE subscriber.
    ///
    /// The queue is the transport's only backlog for a subscription: a message
    /// that does not fit is never awaited on, and the subscriber whose queue is
    /// full is disconnected instead (see `sse::sse`). An unacked delivery of a
    /// process that has not settled stays in the store, so the engine's retry
    /// timer re-sends it to the channel when the client subscribes again.
    pub queue_size: Option<usize>,
}

impl HttpConfig {
    /// The per-subscriber queue capacity, clamped so that neither `0` (a
    /// subscription that can never receive a message) nor an absurd value (the
    /// unbounded backlog the queue exists to prevent) is configurable.
    pub fn queue_size(&self) -> usize {
        self.queue_size
            .unwrap_or(DEFAULT_QUEUE_SIZE)
            .clamp(1, MAX_QUEUE_SIZE)
    }
}
