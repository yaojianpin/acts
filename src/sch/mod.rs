mod cache;
mod consts;
mod context;
mod event;
mod proc;
mod queue;
mod scheduler;
mod state;

#[cfg(test)]
mod tests;

use crate::{Act, Engine, Job, Step, Workflow};
use async_trait::async_trait;
use core::clone::Clone;
use std::sync::Arc;

pub use context::Context;
pub use event::{Event, EventAction, EventData, Message, UserData};
pub use proc::{Matcher, Proc, Task};
pub use scheduler::Scheduler;
pub use state::TaskState;

#[async_trait]
pub trait ActTask: Clone + Send {
    fn prepare(&self, _ctx: &Context) {}
    fn run(&self, ctx: &Context);
    fn post(&self, _ctx: &Context) {}
}

pub trait ActId: Clone {
    fn tid(&self) -> String;
}

pub trait ActState: Clone + Send {
    fn set_state(&self, state: &TaskState);
    fn state(&self) -> TaskState;
}

pub trait ActTime: Clone + Send {
    fn get_state_time(&self) -> u64;
    fn get_end_time(&self) -> u64;
}

impl Engine {
    pub fn on_workflow_start(&self, f: impl Fn(&Workflow) + Send + Sync + 'static) {
        let evt = Event::OnStart(Arc::new(f));
        self.register_event(&evt);
    }

    pub fn on_workflow_complete(&self, f: impl Fn(&Workflow) + Send + Sync + 'static) {
        let evt = Event::OnComplete(Arc::new(f));
        self.register_event(&evt);
    }

    ///  Receive act message
    ///
    /// Example
    /// ```rust
    /// use act::{Engine, Workflow, Vars, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     let workflow = Workflow::new().with_job(|job| {
    ///         job.with_step(|step| {
    ///             step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
    ///         })
    ///     });
    ///     println!("{}", workflow.to_string().unwrap());
    ///     let e = engine.clone();
    ///     engine.on_message(move |msg: &Message| {
    ///         e.close();
    ///         assert_eq!(msg.user, "a");
    ///     });
    ///     engine.push(&workflow);
    ///     engine.start().await;
    /// }
    /// ```
    pub fn on_message(&self, f: impl Fn(&Message) + Send + Sync + 'static) {
        let evt = Event::OnMessage(Arc::new(f));
        self.register_event(&evt);
    }

    pub fn on_job(&self, f: impl Fn(&Job) + Send + Sync + 'static) {
        let evt = Event::OnJob(Arc::new(f));
        self.register_event(&evt);
    }

    pub fn on_step(&self, f: impl Fn(&Step) + Send + Sync + 'static) {
        let evt = Event::OnStep(Arc::new(f));
        self.register_event(&evt);
    }

    pub fn on_act(&self, f: impl Fn(&Act) + Send + Sync + 'static) {
        let evt = Event::OnAct(Arc::new(f));
        self.register_event(&evt);
    }
}
