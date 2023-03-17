use crate::{
    debug,
    sch::{Event, EventData, Proc, Scheduler, Task},
    Message, State, Workflow,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Emitter {
    scher: Arc<Scheduler>,
}

impl Emitter {
    pub(crate) fn new(sch: &Arc<Scheduler>) -> Self {
        Self { scher: sch.clone() }
    }

    pub(crate) fn on_proc(&self, f: impl Fn(&Proc, &EventData) + Send + Sync + 'static) {
        let evt = Event::OnProc(Arc::new(f));
        self.scher.evt().add_event(&evt);
    }

    pub(crate) fn on_task(&self, f: impl Fn(&Task, &EventData) + Send + Sync + 'static) {
        let evt = Event::OnTask(Arc::new(f));
        self.scher.evt().add_event(&evt);
    }

    ///  Receive act message
    ///
    /// Example
    /// ```rust
    /// use acts::{Engine, Workflow, Vars, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     let executor = engine.executor();
    ///     let workflow = Workflow::new().with_job(|job| {
    ///         job.with_step(|step| {
    ///             step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
    ///         })
    ///     });
    ///     println!("{}", workflow.to_string().unwrap());
    ///     let e = engine.clone();
    ///     engine.emitter().on_message(move |msg: &Message| {
    ///         e.close();
    ///         assert_eq!(msg.uid, Some("a".to_string()));
    ///     });
    ///     executor.start(&workflow);
    ///     engine.start().await;
    /// }
    /// ```
    pub fn on_message(&self, f: impl Fn(&Message) + Send + Sync + 'static) {
        let evt = Event::OnMessage(Arc::new(f));
        self.scher.evt().add_event(&evt);
    }

    pub fn on_start(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        let evt = Event::OnStart(Arc::new(f));
        self.scher.evt().add_event(&evt);
    }

    pub fn on_complete(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        let evt = Event::OnComplete(Arc::new(f));
        self.scher.evt().add_event(&evt);
    }

    pub fn on_error(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        let evt = Event::OnError(Arc::new(f));
        self.scher.evt().add_event(&evt);
    }
}
