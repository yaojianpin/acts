use crate::{
    Event, ShareLock,
    event::Message,
    utils::{LaneRouter, shared},
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{
    Mutex,
    mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
};
use tracing::{debug, error, instrument};

pub type ActWorkflowMessageHandle =
    Arc<dyn Fn(Event<Message>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Keyed workflow events. Events are routed to one ordered queue consumer per
/// process (`Message.pid`): handlers of one process run in emission order
/// (never concurrently), while different processes are dispatched
/// concurrently — a slow handler only stalls its own process. Spawning one
/// task per handler per event without the per-process serialization made
/// delivery order nondeterministic, which is why events stay ordered within
/// a process.
enum KeyEvent {
    Start(Message),
    Complete(Message),
    Message(Message),
    Error(Message),
    /// a stored delivery row is re-sent to the single channel it belongs to
    Delivery {
        chan_id: String,
        msg: Message,
    },
}

/// A completed (or errored) process emits no further workflow events, so the
/// worker exits right after delivering the terminal event instead of waiting
/// out the idle timeout. The idle timeout only cleans up processes that never
/// reached a terminal event (aborted/skipped/removed without emission).
fn is_terminal(event: &KeyEvent) -> bool {
    matches!(event, KeyEvent::Complete(_) | KeyEvent::Error(_))
}

/// How long an idle per-process worker stays alive before releasing itself.
const WORKER_IDLE: Duration = Duration::from_secs(30);

type Workers = Arc<RwLock<HashMap<String, UnboundedSender<KeyEvent>>>>;

/// Serializes user event handlers against scheduler-lane work for the same
/// process. Per-pid emitter workers already order handlers among themselves;
/// this gate also preserves the old single-loop barrier between a scheduler
/// job and handlers reacting to its emissions.
#[derive(Clone, Debug)]
pub(crate) struct ProcessGate {
    locks: Arc<Vec<Arc<Mutex<()>>>>,
    router: Arc<LaneRouter>,
}

impl ProcessGate {
    /// A gate with its own router — tests and standalone emitters.
    pub(crate) fn new(count: usize) -> Self {
        Self::with_router(shared(count))
    }

    /// A gate routed by `router`. The lock pool is sized by the router, so
    /// `lane_index` always addresses a real lane and the scheduler queue (built
    /// on this same gate) matches the pool to it.
    pub(crate) fn with_router(router: Arc<LaneRouter>) -> Self {
        Self {
            locks: Arc::new(
                (0..router.lanes())
                    .map(|_| Arc::new(Mutex::new(())))
                    .collect(),
            ),
            router,
        }
    }

    /// This intentionally uses the same lane mapping as the scheduler (the
    /// shared [`LaneRouter`]), so a process's event handlers serialize against
    /// that process's lane.
    pub(crate) fn lane_index(&self, pid: &str) -> usize {
        self.router.lane_of(pid)
    }

    pub(crate) async fn lock(&self, pid: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self.locks[self.lane_index(pid)].clone();
        lock.lock_owned().await
    }
}

/// Spawn the ordered consumer for `pid` and return its sender. Callers insert
/// the sender into `workers` before handing it any event (see [`Emitter::route`]);
/// the worker also uses the same choke point to re-home its queue on exit.
#[allow(clippy::too_many_arguments)]
fn spawn_pid_worker(
    workers: Workers,
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    gate: ProcessGate,
    pid: String,
) -> UnboundedSender<KeyEvent> {
    let (tx, rx) = unbounded_channel();
    tokio::spawn(consume_pid_events(
        workers, starts, completes, messages, errors, gate, pid, rx,
    ));
    tx
}

/// One ordered consumer per process id. Panics of a handler future are
/// isolated: every invocation is spawned and awaited, so a panicking handler
/// neither kills the consumer nor reorders the next event of the process.
#[allow(clippy::too_many_arguments)]
async fn consume_pid_events(
    workers: Workers,
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    gate: ProcessGate,
    pid: String,
    mut rx: UnboundedReceiver<KeyEvent>,
) {
    loop {
        let recv = tokio::time::timeout(WORKER_IDLE, rx.recv()).await;
        let Some(event) = recv.unwrap_or(None) else {
            break;
        };
        let terminal = is_terminal(&event);
        match event {
            KeyEvent::Start(item) => {
                let gate = gate.lock(&item.pid).await;
                dispatch_key_event(&starts, item).await;
                drop(gate);
            }
            KeyEvent::Complete(item) => {
                let gate = gate.lock(&item.pid).await;
                dispatch_key_event(&completes, item).await;
                drop(gate);
            }
            KeyEvent::Message(item) => {
                let gate = gate.lock(&item.pid).await;
                dispatch_key_event(&messages, item).await;
                drop(gate);
            }
            KeyEvent::Error(item) => {
                let gate = gate.lock(&item.pid).await;
                dispatch_key_event(&errors, item).await;
                drop(gate);
            }
            KeyEvent::Delivery { chan_id, msg } => {
                let gate = gate.lock(&msg.pid).await;
                dispatch_delivery(&messages, &chan_id, msg).await;
                drop(gate);
            }
        }
        if terminal {
            break;
        }
    }
    // Exit (terminal event, or the idle timeout). Close the routing gate under
    // the write lock first: `route` holds the read lock across its `send`, so
    // once this removal wins no event can still be accepted by this worker.
    // Events already accepted but not yet dequeued are then re-homed to a
    // fresh worker instead of dropped with `rx` — covering the window where a
    // delivery (e.g. a retry-timer re-send) or completion arrives right after
    // the terminal event. Re-homing under the same lock keeps their order
    // ahead of any event routed after the removal.
    {
        let mut map = workers.write();
        map.remove(&pid);
        let mut pending = Vec::new();
        while let Ok(event) = rx.try_recv() {
            pending.push(event);
        }
        if !pending.is_empty() {
            let tx = spawn_pid_worker(
                workers.clone(),
                starts.clone(),
                completes.clone(),
                messages.clone(),
                errors.clone(),
                gate.clone(),
                pid.clone(),
            );
            for event in pending {
                let _ = tx.send(event);
            }
            map.insert(pid.clone(), tx);
        }
    }
}

/// Run one handler invocation. The spawn isolates a panicking handler from
/// the consumer task; awaiting the handle keeps the per-process order.
async fn run_handle(handle: ActWorkflowMessageHandle, event: Event<Message>) {
    let result = tokio::spawn(async move { handle(event).await }).await;
    if let Err(payload) = result
        && payload.is_panic()
    {
        error!("event handler panicked");
    }
}

/// Deliver a stored delivery row to the single channel handler it belongs to.
/// When no handler is registered under the channel (it unsubscribed) the row
/// is dropped — it stays in the store and will be retried later.
async fn dispatch_delivery(
    handlers: &ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    chan_id: &str,
    item: Message,
) {
    let Some(handle) = handlers.read().get(chan_id).cloned() else {
        debug!(chan = %chan_id, "delivery channel handler not found");
        return;
    };
    let event = Event::from_inner(item);
    run_handle(handle, event).await;
}

/// Invoke every registered handler for `item`, in registration order per
/// event, each isolated by its own task.
async fn dispatch_key_event(
    handlers: &ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    item: Message,
) {
    let handles: Vec<_> = handlers.read().values().cloned().collect();
    for handle in handles {
        let event = Event::from_inner(item.clone());
        run_handle(handle, event).await;
    }
}

pub struct Emitter {
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    workers: Workers,
    process_gate: ProcessGate,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emitter").finish()
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Emitter {
    pub fn new() -> Self {
        Self::with_process_gate(ProcessGate::new(1))
    }

    pub(crate) fn with_process_gate(process_gate: ProcessGate) -> Self {
        Self {
            messages: Arc::new(RwLock::new(HashMap::new())),
            starts: Arc::new(RwLock::new(HashMap::new())),
            completes: Arc::new(RwLock::new(HashMap::new())),
            errors: Arc::new(RwLock::new(HashMap::new())),
            process_gate,
            workers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        self.messages.write().clear();
        self.starts.write().clear();
        self.completes.write().clear();
        self.errors.write().clear();
    }

    pub fn on_message<F, Fut>(&self, key: &str, f: F)
    where
        F: Fn(Event<Message>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let f: ActWorkflowMessageHandle = Arc::new(move |e| Box::pin(f(e)));
        self.messages
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_start<F, Fut>(&self, key: &str, f: F)
    where
        F: Fn(Event<Message>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let f: ActWorkflowMessageHandle = Arc::new(move |e| Box::pin(f(e)));
        self.starts
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_complete<F, Fut>(&self, key: &str, f: F)
    where
        F: Fn(Event<Message>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let f: ActWorkflowMessageHandle = Arc::new(move |e| Box::pin(f(e)));
        self.completes
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_error<F, Fut>(&self, key: &str, f: F)
    where
        F: Fn(Event<Message>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let f: ActWorkflowMessageHandle = Arc::new(move |e| Box::pin(f(e)));
        self.errors
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    /// Route one workflow event to the ordered consumer of its process,
    /// starting the consumer on first use.
    ///
    /// A send that fails means that sender's receiver is gone — its worker
    /// task ended without the exit path that replaces the entry (the task was
    /// dropped, e.g. the runtime was aborted) — so the entry is never written
    /// into and left: it is replaced by a fresh worker that the event is then
    /// handed to.
    fn route(&self, mut event: KeyEvent) {
        let pid = match &event {
            KeyEvent::Start(m)
            | KeyEvent::Complete(m)
            | KeyEvent::Message(m)
            | KeyEvent::Error(m) => m.pid.clone(),
            KeyEvent::Delivery { msg, .. } => msg.pid.clone(),
        };
        {
            let workers = self.workers.read();
            if let Some(tx) = workers.get(&pid) {
                match tx.send(event) {
                    Ok(()) => return,
                    // the worker exited between the read and the send (idle
                    // timeout or terminal event) — re-create it below
                    Err(err) => event = err.0,
                }
            }
        }
        let mut workers = self.workers.write();
        if let Some(tx) = workers.get(&pid) {
            match tx.send(event) {
                Ok(()) => return,
                // a stale entry: this sender's worker is gone without having
                // re-homed its queue, so the entry must be replaced instead of
                // accepting an event nobody will ever read
                Err(err) => event = err.0,
            }
        }
        let tx = spawn_pid_worker(
            self.workers.clone(),
            self.starts.clone(),
            self.completes.clone(),
            self.messages.clone(),
            self.errors.clone(),
            self.process_gate(),
            pid.clone(),
        );
        workers.insert(pid, tx.clone());
        drop(workers);
        // the receiver was created one line above: it is gone only if the
        // just-spawned task never got to run (no runtime left), and then there
        // is no queue left to re-home the event to either
        let _ = tx.send(event);
    }

    pub(crate) fn process_gate(&self) -> ProcessGate {
        self.process_gate.clone()
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_start_event(&self, state: &Message) {
        debug!(state = %state.state, "start event emitted");
        self.route(KeyEvent::Start(state.clone()));
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_complete_event(&self, state: &Message) {
        debug!(state = %state.state, "complete event emitted");
        self.route(KeyEvent::Complete(state.clone()));
    }

    #[instrument(skip(self, msg), fields(pid = %msg.pid, tid = %msg.tid, mid = %msg.mid))]
    pub fn emit_message(&self, msg: &Message) {
        debug!("message emitted");
        self.route(KeyEvent::Message(msg.clone()));
    }

    /// Re-send a stored delivery row only to the channel it belongs to
    /// (`chan_id`), not to every matching channel handler.
    #[instrument(skip(self, msg), fields(pid = %msg.pid, tid = %msg.tid, mid = %msg.mid))]
    pub fn emit_delivery(&self, chan_id: &str, msg: &Message) {
        debug!(chan = %chan_id, "delivery emitted");
        self.route(KeyEvent::Delivery {
            chan_id: chan_id.to_string(),
            msg: msg.clone(),
        });
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_error(&self, state: &Message) {
        debug!(state = %state.state, "error event emitted");
        self.route(KeyEvent::Error(state.clone()));
    }

    pub fn remove(&self, key: &str) {
        let mut starts = self.starts.write();
        if starts.contains_key(key) {
            starts.remove(key);
        }

        let mut completes = self.completes.write();
        if completes.contains_key(key) {
            completes.remove(key);
        }

        let mut errors = self.errors.write();
        if errors.contains_key(key) {
            errors.remove(key);
        }

        let mut messages = self.messages.write();
        if messages.contains_key(key) {
            messages.remove(key);
        }
    }

    pub(crate) fn close(&self) {
        self.messages.write().clear();
        self.starts.write().clear();
        self.completes.write().clear();
        self.errors.write().clear();
        // Dropping every sender closes the per-process queues; the workers
        // exit on the closed channel and remove themselves.
        self.workers.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vars;

    fn message(pid: &str) -> Message {
        Message {
            pid: pid.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_emitter_debugs_and_a_reset_drops_every_handler() {
        let emitter = Emitter::new();
        assert_eq!(format!("{emitter:?}"), "Emitter");
        let emitter = Emitter::default();
        emitter.on_message("k", |_| async {});
        emitter.on_start("k", |_| async {});
        emitter.on_complete("k", |_| async {});
        emitter.on_error("k", |_| async {});
        emitter.reset();
        // after the reset the handler maps are empty, so the event has no
        // receiver to reach
        emitter.emit_message(&message("p1"));
        emitter.emit_start_event(&message("p1"));
        emitter.emit_complete_event(&message("p1"));
        emitter.emit_error(&message("p1"));
    }

    #[test]
    fn a_process_gate_sizes_its_lock_pool_from_the_router() {
        let gate = ProcessGate::new(4);
        assert!(gate.lane_index("pid") < 4);
        assert_eq!(format!("{gate:?}"), format!("{gate:?}"));
    }

    #[tokio::test]
    async fn a_delivery_without_a_channel_handler_is_dropped() {
        let handlers: ShareLock<HashMap<String, ActWorkflowMessageHandle>> =
            Arc::new(RwLock::new(HashMap::new()));
        dispatch_delivery(&handlers, "unsubscribed", message("p1")).await;
    }

    #[tokio::test]
    async fn remove_drops_a_handler_from_every_map() {
        let emitter = Emitter::new();
        emitter.on_start("k", |_| async {});
        emitter.on_complete("k", |_| async {});
        emitter.on_error("k", |_| async {});
        emitter.on_message("k", |_| async {});
        emitter.remove("k");
        // and the second removal takes the "not there" path
        emitter.remove("k");
        assert!(emitter.starts.read().is_empty());
        assert!(emitter.completes.read().is_empty());
        assert!(emitter.errors.read().is_empty());
        assert!(emitter.messages.read().is_empty());
    }

    #[tokio::test]
    async fn a_delivery_written_in_both_maps_keeps_its_scope() {
        // A `Delivery` is routed by its message's pid, not by the channel.
        let mut msg = message("p1");
        msg.inputs = Vars::new().with("options", Vars::new().with("tag", "vip"));
        let emitter = Emitter::new();
        emitter.emit_delivery("chan-1", &msg);
    }

    /// Two routes race on a stale (closed) worker entry: the first to reach
    /// the write lock replaces it, and the second finds the fresh sender and
    /// hands its event over instead of spawning a third worker.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_route_finds_the_replacement_worker() {
        let emitter = Arc::new(Emitter::new());
        let (tx, rx) = unbounded_channel();
        drop(rx);
        emitter.workers.write().insert("race".to_string(), tx);

        let handle = tokio::runtime::Handle::current();
        // Hold the read lock so neither sender can reach the write lock
        // before both have observed the closed sender.
        let read = emitter.workers.read();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let emitter = emitter.clone();
                let barrier = barrier.clone();
                let handle = handle.clone();
                std::thread::spawn(move || {
                    let _enter = handle.enter();
                    barrier.wait();
                    emitter.emit_message(&message("race"));
                })
            })
            .collect();
        barrier.wait();
        std::thread::sleep(Duration::from_millis(100));
        drop(read);
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
