use crate::sch::TaskState;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Proc {
    pub id: String,
    pub pid: String,
    pub model: String,
    pub state: TaskState,
    pub vars: String,
}
