use std::sync::Arc;

use crate::{sch::Scheduler, Event, Message, WorkflowState};

/// Just a export struct for the event::Emitter
///
pub struct Emitter {
    scher: Arc<Scheduler>,
}

impl Emitter {
    pub fn new(scher: &Arc<Scheduler>) -> Self {
        Self {
            scher: scher.clone(),
        }
    }

    ///  Receive act message
    ///
    /// Example
    /// ```rust,no_run
    /// use acts::{Engine, Act, Workflow, Vars, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     let workflow = Workflow::new().with_id("m1").with_step(|step| {
    ///             step.with_id("step1").with_act(Act::req(|act| act.with_id("act1")))
    ///     });
    ///
    ///     engine.emitter().on_message(move |e| {
    ///         if e.r#type == "req" {
    ///             println!("act message: id={} state={} inputs={:?} outputs={:?}", e.id, e.state, e.inputs, e.outputs);
    ///         }
    ///     });
    ///
    ///     engine.manager().deploy(&workflow).expect("fail to deploy workflow");
    ///     let mut vars = Vars::new();
    ///     vars.insert("pid".into(), "w1".into());
    ///     engine.executor().start(
    ///        &workflow.id,
    ///        &vars,
    ///    );
    /// }
    /// ```
    pub fn on_message(&self, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        self.scher.emitter().on_message(f);
    }

    pub fn on_start(&self, f: impl Fn(&Event<WorkflowState>) + Send + Sync + 'static) {
        self.scher.emitter().on_start(f);
    }

    pub fn on_complete(&self, f: impl Fn(&Event<WorkflowState>) + Send + Sync + 'static) {
        self.scher.emitter().on_complete(f);
    }

    pub fn on_error(&self, f: impl Fn(&Event<WorkflowState>) + Send + Sync + 'static) {
        self.scher.emitter().on_error(f);
    }
}
