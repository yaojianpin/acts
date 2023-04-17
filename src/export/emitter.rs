use crate::{event, sch::Scheduler, Message, State, Workflow};
use std::sync::Arc;

/// Just a export struct for the event::Emitter
///
pub struct Emitter {
    evt: Arc<event::Emitter>,
}

impl Emitter {
    pub fn new(scher: &Arc<Scheduler>) -> Self {
        Self {
            evt: scher.emitter(),
        }
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
        self.evt.on_message(f);
    }

    pub fn on_start(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        self.evt.on_start(f);
    }

    pub fn on_complete(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        self.evt.on_complete(f);
    }

    pub fn on_error(&self, f: impl Fn(&State<Workflow>) + Send + Sync + 'static) {
        self.evt.on_error(f);
    }
}
