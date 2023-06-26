use crate::{event::EventAction, sch::TaskState, utils, ActValue, Vars};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct WorkflowState {
    pub pid: String,
    pub mid: String,
    pub event: EventAction,
    pub state: TaskState,
    pub start_time: i64,
    pub end_time: i64,

    pub outputs: Vars,
    // pub(crate) proc: Arc<Proc>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ActionState {
    pub start_time: i64,
    pub end_time: i64,

    outputs: Vars,
    // pub(crate) proc: Arc<Proc>,
}

impl WorkflowState {
    /// Get the workflow output vars
    pub fn outputs(&self) -> &Vars {
        &self.outputs
    }

    /// How many time(million seconds) did a workflow cost
    pub fn cost(&self) -> i64 {
        if self.state.is_completed() {
            return self.end_time - self.start_time;
        }

        0
    }
}

impl ActionState {
    pub fn begin() -> Self {
        Self {
            start_time: utils::time::time(),
            end_time: 0,
            outputs: Vars::new(),
        }
    }

    pub fn end(&mut self) {
        self.end_time = utils::time::time()
    }

    /// Get the workflow output vars
    pub fn outputs(&self) -> &Vars {
        &self.outputs
    }

    pub fn insert(&mut self, key: &str, value: ActValue) {
        self.outputs.insert(key.to_string(), value);
    }

    /// How many time(million seconds) did a workflow cost
    pub fn cost(&self) -> i64 {
        self.end_time - self.start_time
    }
}

impl ActionState {
    // Print the workflow tree
    // pub fn tree(&self) -> ActResult<()> {
    //     let model = self.proc.workflow();
    //     model.print_tree()
    // }

    // pub fn trace(&self) -> ActResult<()> {
    //     let model = self.proc.workflow();
    //     utils::log::print_tree(&model)
    // }

    // pub fn tasks(&self) -> ActResult<Vec<TaskInfo>> {
    //     let tasks: Vec<_> = self.proc.tasks().iter().map(|task| task.into()).collect();
    //     Ok(tasks)
    // }

    // pub fn acts(&self) -> ActResult<Vec<ActInfo>> {
    //     let tasks: Vec<_> = self.proc.acts().iter().map(|act| act.into()).collect();
    //     Ok(tasks)
    // }
}

impl std::fmt::Debug for ActionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("start_time", &self.start_time)
            .field("end_time", &self.end_time)
            .field("outputs", &self.outputs)
            .finish()
    }
}

impl std::fmt::Debug for WorkflowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("pid", &self.pid)
            .field("state", &self.state)
            .field("start_time", &self.start_time)
            .field("end_time", &self.end_time)
            .field("outputs", &self.outputs)
            .finish()
    }
}
