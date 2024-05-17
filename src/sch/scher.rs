use crate::{
    config::Config,
    event::{Emitter, TaskExtra},
    sch::{
        queue::{Queue, Signal},
        Proc, Task,
    },
    Engine, Event, Result,
};
use std::sync::{Arc, Mutex};
use tracing::debug;

#[derive(Clone)]
pub struct Scheduler {
    queue: Arc<Queue>,
    emitter: Arc<Emitter>,
    closed: Arc<Mutex<bool>>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler").finish()
    }
}

impl Scheduler {
    pub fn new() -> Arc<Self> {
        Scheduler::new_with(&Config::default())
    }

    pub fn new_with(_options: &Config) -> Arc<Self> {
        let scher = Arc::new(Self {
            queue: Queue::new(),
            emitter: Arc::new(Emitter::new()),
            closed: Arc::new(Mutex::new(false)),
        });

        scher
    }

    pub fn init(self: &Arc<Self>, _engine: &Engine) {
        debug!("sch::init");
    }

    pub fn push(&self, task: &Arc<Task>) {
        debug!("sch::push  task={:?}", task);
        self.queue.send(&Signal::Task(task.clone()));
    }

    pub async fn next(self: &Arc<Self>) -> bool {
        if let Some(signal) = self.queue.next().await {
            debug!("next: {:?}", signal);
            match signal {
                Signal::Task(task) => {
                    let ctx = &task.create_context();
                    task.exec(ctx).unwrap_or_else(|err| {
                        eprintln!("error: {err}");
                        task.set_err(&err.into());
                        let _ = ctx.emit_error();
                    });
                }
                Signal::Terminal => {
                    *self.closed.lock().unwrap() = true;
                    return false;
                }
            }
        }

        true
    }

    pub fn close(&self) {
        debug!("sch::close");
        self.queue.terminate();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.lock().unwrap().clone()
    }

    pub fn on_proc(&self, f: impl Fn(&Event<Arc<Proc>>) + Send + Sync + 'static) {
        self.emitter.on_proc(f)
    }

    pub fn on_task(&self, f: impl Fn(&Event<Arc<Task>, TaskExtra>) + Send + Sync + 'static) {
        self.emitter.on_task(f)
    }

    pub fn emit_proc_event(&self, proc: &Arc<Proc>) {
        self.emitter.emit_proc_event(proc)
    }

    pub fn emit_task_event(&self, task: &Arc<Task>) -> Result<()> {
        self.emitter.emit_task_event(task)
    }
}
