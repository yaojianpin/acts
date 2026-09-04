use crate::{
    Event, Result, ShareLock,
    event::Message,
    scheduler::{Process, Task},
};
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, error, instrument};

use super::TaskExtra;

pub type ActWorkflowMessageHandle = Arc<dyn Fn(&Event<Message>) + Send + Sync>;
pub type ProcHandle = Arc<dyn Fn(&Event<Arc<Process>>) + Send + Sync>;
pub type TaskHandle = Arc<dyn Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync>;

/// Keyed workflow events, delivered to handlers in FIFO order by a single
/// queue consumer. Spawning one task per handler per event made delivery
/// order nondeterministic; the queue guarantees emission order.
enum KeyEvent {
    Start(Message),
    Complete(Message),
    Message(Message),
    Error(Message),
}

async fn consume_key_events(
    mut rx: UnboundedReceiver<KeyEvent>,
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
) {
    while let Some(event) = rx.recv().await {
        match event {
            KeyEvent::Start(item) => dispatch_key_event(&starts, item),
            KeyEvent::Complete(item) => dispatch_key_event(&completes, item),
            KeyEvent::Message(item) => dispatch_key_event(&messages, item),
            KeyEvent::Error(item) => dispatch_key_event(&errors, item),
        }
    }
}

/// Invoke every registered handler for `item`. A panicking handler must not
/// stop delivery to the remaining handlers (the previous per-handler spawn
/// isolated panics), so each call is guarded by `catch_unwind`.
fn dispatch_key_event(
    handlers: &ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    item: Message,
) {
    let handles: Vec<_> = handlers.read().values().cloned().collect();
    for handle in handles {
        let event = Event::from_inner(item.clone());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (handle)(&event)));
        if let Err(payload) = result {
            let panic = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            error!(panic = %panic, "event handler panicked");
        }
    }
}

pub struct Emitter {
    starts: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    completes: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    messages: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,
    errors: ShareLock<HashMap<String, ActWorkflowMessageHandle>>,

    procs: ShareLock<Vec<ProcHandle>>,
    tasks: ShareLock<Vec<TaskHandle>>,

    queue: UnboundedSender<KeyEvent>,
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
        let messages = Arc::new(RwLock::new(HashMap::new()));
        let starts = Arc::new(RwLock::new(HashMap::new()));
        let completes = Arc::new(RwLock::new(HashMap::new()));
        let errors = Arc::new(RwLock::new(HashMap::new()));
        let (queue, rx) = unbounded_channel();
        tokio::spawn(consume_key_events(
            rx,
            starts.clone(),
            completes.clone(),
            messages.clone(),
            errors.clone(),
        ));
        Self {
            messages,
            starts,
            completes,
            errors,
            procs: Arc::new(RwLock::new(Vec::new())),
            tasks: Arc::new(RwLock::new(Vec::new())),
            queue,
        }
    }

    #[cfg(test)]
    pub fn reset(&self) {
        self.messages.write().clear();
        self.starts.write().clear();
        self.completes.write().clear();
        self.errors.write().clear();
    }

    pub fn on_message(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.messages
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_start(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.starts
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_complete(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.completes
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_error(&self, key: &str, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let f = Arc::new(f);
        self.errors
            .write()
            .entry(key.to_string())
            .and_modify(|v| *v = f.clone())
            .or_insert(f);
    }

    pub fn on_proc(&self, f: impl Fn(&Event<Arc<Process>>) + Send + Sync + 'static) {
        self.procs.write().push(Arc::new(f));
    }

    pub fn on_task(&self, f: impl Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync + 'static) {
        self.tasks.write().push(Arc::new(f));
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub fn emit_proc_event(&self, proc: &Arc<Process>) {
        debug!("proc event emitted");
        let handlers = self.procs.read();
        let e = &Event::new(proc);
        for handle in handlers.iter() {
            (handle)(e);
        }
    }

    pub fn emit_task_event(&self, task: &Arc<Task>) -> Result<()> {
        self.emit_task_event_with_extra(task, true)
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub fn emit_task_event_with_extra(&self, task: &Arc<Task>, emit_message: bool) -> Result<()> {
        debug!("task event emitted");
        let handlers = self.tasks.read();
        let e = &Event::new_with_extra(task, &TaskExtra { emit_message });
        for handle in handlers.iter() {
            (handle)(e);
        }

        Ok(())
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_start_event(&self, state: &Message) {
        debug!(state = %state.state, "start event emitted");
        let _ = self.queue.send(KeyEvent::Start(state.clone()));
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_complete_event(&self, state: &Message) {
        debug!(state = %state.state, "complete event emitted");
        let _ = self.queue.send(KeyEvent::Complete(state.clone()));
    }

    #[instrument(skip(self, msg), fields(pid = %msg.pid, tid = %msg.tid, mid = %msg.mid))]
    pub fn emit_message(&self, msg: &Message) {
        debug!("message emitted");
        let _ = self.queue.send(KeyEvent::Message(msg.clone()));
    }

    #[instrument(skip(self, state), fields(pid = %state.pid, tid = %state.tid, mid = %state.mid))]
    pub fn emit_error(&self, state: &Message) {
        debug!(state = %state.state, "error event emitted");
        let _ = self.queue.send(KeyEvent::Error(state.clone()));
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
    }
}
