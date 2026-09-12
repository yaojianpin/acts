use crate::{
    Event, Result, ShareLock,
    event::Message,
    scheduler::{Process, Task},
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

use super::TaskExtra;

pub type ActWorkflowMessageHandle =
    Arc<dyn Fn(Event<Message>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
pub type ProcHandle =
    Arc<dyn Fn(Event<Arc<Process>>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
pub type TaskHandle = Arc<
    dyn Fn(Event<Arc<Task>, TaskExtra>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

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
}

impl ProcessGate {
    pub(crate) fn new(count: usize) -> Self {
        Self {
            locks: Arc::new(
                (0..count.max(1))
                    .map(|_| Arc::new(Mutex::new(())))
                    .collect(),
            ),
        }
    }

    /// This intentionally uses the same FNV-1a lane mapping as the scheduler,
    /// so a process's event handlers serialize against that process's lane.
    pub(crate) fn lane_index(&self, pid: &str) -> usize {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in pid.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        (hash as usize) % self.locks.len()
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

async fn run_proc_handle(handle: ProcHandle, event: Event<Arc<Process>>) {
    let result = tokio::spawn(async move { handle(event).await }).await;
    if let Err(payload) = result
        && payload.is_panic()
    {
        error!("proc handler panicked");
    }
}

async fn run_task_handle(handle: TaskHandle, event: Event<Arc<Task>, TaskExtra>) {
    let result = tokio::spawn(async move { handle(event).await }).await;
    if let Err(payload) = result
        && payload.is_panic()
    {
        error!("task handler panicked");
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

    procs: ShareLock<Vec<ProcHandle>>,
    tasks: ShareLock<Vec<TaskHandle>>,

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
            procs: Arc::new(RwLock::new(Vec::new())),
            tasks: Arc::new(RwLock::new(Vec::new())),
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

    pub fn on_proc<F, Fut>(&self, f: F)
    where
        F: Fn(Event<Arc<Process>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let f: ProcHandle = Arc::new(move |e| Box::pin(f(e)));
        self.procs.write().push(f);
    }

    pub fn on_task<F, Fut>(&self, f: F)
    where
        F: Fn(Event<Arc<Task>, TaskExtra>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let f: TaskHandle = Arc::new(move |e| Box::pin(f(e)));
        self.tasks.write().push(f);
    }

    /// Route one workflow event to the ordered consumer of its process,
    /// starting the consumer on first use.
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
            let _ = tx.send(event);
            return;
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
        let _ = tx.send(event);
    }

    pub(crate) fn process_gate(&self) -> ProcessGate {
        self.process_gate.clone()
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub async fn emit_proc_event(&self, proc: &Arc<Process>) {
        debug!("proc event emitted");
        let handlers: Vec<ProcHandle> = self.procs.read().clone();
        let proc = proc.clone();
        for handle in handlers {
            run_proc_handle(handle, Event::new(&proc)).await;
        }
    }

    pub async fn emit_task_event(&self, task: &Arc<Task>) -> Result<()> {
        self.emit_task_event_with_extra(task, true).await
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub async fn emit_task_event_with_extra(
        &self,
        task: &Arc<Task>,
        emit_message: bool,
    ) -> Result<()> {
        debug!("task event emitted");
        let handlers: Vec<TaskHandle> = self.tasks.read().clone();
        let task = task.clone();
        for handle in handlers {
            let extra = TaskExtra { emit_message };
            run_task_handle(handle, Event::new_with_extra(&task, &extra)).await;
        }
        Ok(())
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
        self.procs.write().clear();
        self.tasks.write().clear();
        // Dropping every sender closes the per-process queues; the workers
        // exit on the closed channel and remove themselves.
        self.workers.write().clear();
    }
}
