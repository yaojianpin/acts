use crate::{
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
    /// use acts::{ActionOptions, Engine, Workflow, Vars, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     engine.start();
    ///
    ///     let workflow = Workflow::new().with_id("m1").with_job(|job| {
    ///         job.with_id("job1").with_step(|step| {
    ///             step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
    ///         })
    ///     });
    ///
    ///     engine.emitter().on_message(move |msg: &Message| {
    ///         assert_eq!(msg.uid, Some("a".to_string()));
    ///     });
    ///
    ///     let executor = engine.executor();
    ///     executor.deploy(&workflow).expect("fail to deploy workflow");
    ///     executor.start(
    ///        &workflow.id,
    ///        ActionOptions {
    ///            biz_id: Some("w1".to_string()),
    ///            ..Default::default()
    ///        },
    ///    );
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
