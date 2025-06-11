mod act_executor;
mod event_executor;
mod message_executor;
mod model_executor;
mod package_executor;
mod process_executor;
mod task_executor;

use crate::scheduler::Runtime;
use std::sync::Arc;

#[derive(Clone)]
pub struct Executor {
    msg: message_executor::MessageExecutor,
    act: act_executor::ActExecutor,
    model: model_executor::ModelExecutor,
    proc: process_executor::ProcessExecutor,
    task: task_executor::TaskExecutor,
    pack: package_executor::PackageExecutor,
    evt: event_executor::EventExecutor,
}

impl Executor {
    pub(crate) fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            msg: message_executor::MessageExecutor::new(rt),
            act: act_executor::ActExecutor::new(rt),
            model: model_executor::ModelExecutor::new(rt),
            proc: process_executor::ProcessExecutor::new(rt),
            task: task_executor::TaskExecutor::new(rt),
            pack: package_executor::PackageExecutor::new(rt),
            evt: event_executor::EventExecutor::new(rt),
        }
    }

    /// executor for related message functions
    pub fn msg(&self) -> &message_executor::MessageExecutor {
        &self.msg
    }

    /// executor for related act operations
    /// such as 'complete', 'back', 'cancel' ..
    pub fn act(&self) -> &act_executor::ActExecutor {
        &self.act
    }

    /// executor for related model functions
    pub fn model(&self) -> &model_executor::ModelExecutor {
        &self.model
    }

    /// executor for related process functions
    pub fn proc(&self) -> &process_executor::ProcessExecutor {
        &self.proc
    }

    /// executor for related task functions
    pub fn task(&self) -> &task_executor::TaskExecutor {
        &self.task
    }

    /// executor for related package functions
    pub fn pack(&self) -> &package_executor::PackageExecutor {
        &self.pack
    }

    /// executor for related event functions
    pub fn evt(&self) -> &event_executor::EventExecutor {
        &self.evt
    }
}
