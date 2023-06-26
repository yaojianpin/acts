use crate::{event, sch::Scheduler, Message, WorkflowState};
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
    /// use acts::{Engine, Workflow, Vars, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     engine.start();
    ///
    ///     let workflow = Workflow::new().with_id("m1").with_job(|job| {
    ///         job.with_id("job1").with_step(|step| {
    ///             step.with_subject(|sub| sub.with_matcher("any").with_cands(r#"["a"]"#))
    ///         })
    ///     });
    ///
    ///     engine.emitter().on_message(move |msg: &Message| {
    ///         if let Some(msg) = msg.as_user_message() {
    ///             println!("act message: aid={} event={} uid={}", msg.aid, msg.event, msg.uid);
    ///         }
    ///     });
    ///
    ///     engine.manager().deploy(&workflow).expect("fail to deploy workflow");
    ///     let mut vars = Vars::new();
    ///     vars.insert("biz_id".into(), "w1".into());
    ///     engine.executor().start(
    ///        &workflow.id,
    ///        &vars,
    ///    );
    /// }
    /// ```
    pub fn on_message(&self, f: impl Fn(&Message) + Send + Sync + 'static) {
        self.evt.on_message(f);
    }

    pub fn on_start(&self, f: impl Fn(&WorkflowState) + Send + Sync + 'static) {
        self.evt.on_start(f);
    }

    pub fn on_complete(&self, f: impl Fn(&WorkflowState) + Send + Sync + 'static) {
        self.evt.on_complete(f);
    }

    pub fn on_error(&self, f: impl Fn(&WorkflowState) + Send + Sync + 'static) {
        self.evt.on_error(f);
    }
}
