mod act;
mod model;
mod msg;
mod pack;
mod proc;
mod task;

use crate::sch::Runtime;
use std::sync::Arc;

#[derive(Clone)]
pub struct Executor {
    msg: msg::MessageExecutor,
    act: act::ActExecutor,
    model: model::ModelExecutor,
    proc: proc::ProcExecutor,
    task: task::TaskExecutor,
    pack: pack::PackageExecutor,
}

impl Executor {
    pub(crate) fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            msg: msg::MessageExecutor::new(rt),
            act: act::ActExecutor::new(rt),
            model: model::ModelExecutor::new(rt),
            proc: proc::ProcExecutor::new(rt),
            task: task::TaskExecutor::new(rt),
            pack: pack::PackageExecutor::new(rt),
        }
    }

    /// executor for related message functions
    pub fn msg(&self) -> &msg::MessageExecutor {
        &self.msg
    }

    /// executor for related act operations
    /// such as 'complete', 'back', 'cancel' ..
    pub fn act(&self) -> &act::ActExecutor {
        &self.act
    }

    /// executor for related model functions
    pub fn model(&self) -> &model::ModelExecutor {
        &self.model
    }

    /// executor for related proc functions
    pub fn proc(&self) -> &proc::ProcExecutor {
        &self.proc
    }

    /// executor for related task functions
    pub fn task(&self) -> &task::TaskExecutor {
        &self.task
    }

    /// executor for related package functions
    pub fn pack(&self) -> &pack::PackageExecutor {
        &self.pack
    }
}
