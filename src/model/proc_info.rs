use crate::{sch::TaskState, Vars};

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: String,
    pub name: String,
    pub model_id: String,
    pub state: TaskState,
    pub start_time: i64,
    pub end_time: i64,
    pub vars: Vars,
}
