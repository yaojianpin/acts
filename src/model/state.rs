use crate::{sch::TaskState, utils, ActResult, ModelBase, Vars, Workflow};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct State<T> {
    pub pid: String,
    pub node: Arc<T>,
    pub state: TaskState,
    pub start_time: i64,
    pub end_time: i64,
    pub outputs: Vars,
}

impl<T> ModelBase for State<T>
where
    T: ModelBase,
{
    fn id(&self) -> &str {
        self.node.id()
    }
}

impl<T> State<T>
where
    T: ModelBase,
{
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

impl State<Workflow> {
    pub fn pid(&self) -> &str {
        &self.pid
    }
    /// Print the workflow tree
    pub fn print_tree(&self) -> ActResult<()> {
        self.node.print_tree()
    }

    pub fn trace(&self) -> ActResult<()> {
        utils::log::print_tree(&self.node)
    }
}
