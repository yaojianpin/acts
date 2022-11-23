use crate::{sch::TaskState, utils, ShareLock, Vars};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub struct Message {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub user: String,
    pub create_time: i64,
    pub data: Option<UserData>,
    state: ShareLock<TaskState>,
}

impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!(
            "Message{{ id:{}, pid:{}, tid:{}, user:{} }}",
            self.id, self.pid, self.tid, self.user
        ))
    }
}

#[derive(Debug, Default, Clone)]
pub struct UserData {
    pub user: String,
    pub vars: Vars,
    pub action: String,
}

impl Message {
    pub fn new(pid: &str, tid: &str, user: &str, data: Option<UserData>) -> Self {
        let id = utils::Id::new(pid, tid);
        Self {
            id: id.id(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            user: user.to_string(),
            create_time: utils::time::time(),
            data,
            state: Arc::new(RwLock::new(TaskState::None)),
        }
    }

    pub fn id(&self) -> String {
        self.id.clone()
    }

    pub fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    pub fn set_state(&self, state: TaskState) {
        *self.state.write().unwrap() = state;
    }
}
