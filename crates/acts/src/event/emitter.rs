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
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
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

/// One ordered consumer per process id. Panics of a handler future are
/// isolated: every invocation is spawned and awaited, so a panicking handler
/// neither kills the consumer nor reorders the next event of the process.
async fn consume_pid_events(
    workers: Workers,
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
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
            KeyEvent::Start(item) => dispatch_key_event(&starts, item).await,
            KeyEvent::Complete(item) => dispatch_key_event(&completes, item).await,
            KeyEvent::Message(item) => dispatch_key_event(&messages, item).await,
            KeyEvent::Error(item) => dispatch_key_event(&errors, item).await,
            KeyEvent::Delivery { chan_id, msg } => {
                dispatch_delivery(&messages, &chan_id, msg).await;
            }
        }
        if terminal {
            break;
        }
    }
    workers.write().remove(&pid);
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
        Self {
            messages: Arc::new(RwLock::new(HashMap::new())),
            starts: Arc::new(RwLock::new(HashMap::new())),
            completes: Arc::new(RwLock::new(HashMap::new())),
            errors: Arc::new(RwLock::new(HashMap::new())),
            procs: Arc::new(RwLock::new(Vec::new())),
            tasks: Arc::new(RwLock::new(Vec::new())),
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
        let (tx, rx) = unbounded_channel();
        workers.insert(pid.clone(), tx.clone());
        drop(workers);
        tokio::spawn(consume_pid_events(
            self.workers.clone(),
            self.starts.clone(),
            self.completes.clone(),
            self.messages.clone(),
            self.errors.clone(),
            pid,
            rx,
        ));
        let _ = tx.send(event);
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
