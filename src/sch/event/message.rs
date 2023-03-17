use crate::{sch::TaskState, utils, ShareLock, Vars};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub struct Message {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub create_time: i64,
    pub update_time: i64,
    pub uid: Option<String>,
    pub vars: Vars,
    state: ShareLock<TaskState>,
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    // pub id: Option<String>,
    pub pid: String,
    pub uid: String,
    pub action: String,
    pub options: Option<ActionOptions>,
}

#[derive(Debug, Default, Clone)]
pub struct ActionOptions {
    pub vars: Vars,
    pub to: Option<String>,
}

impl Message {
    pub fn new(pid: &str, tid: &str, uid: Option<String>, vars: Vars) -> Self {
        let id = utils::Id::new(pid, tid);
        Self {
            id: id.id(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            uid,
            create_time: utils::time::time(),
            update_time: 0,
            vars,
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

impl UserMessage {
    pub fn new(pid: &str, uid: &str, action: &str, options: Option<ActionOptions>) -> Self {
        Self {
            pid: pid.to_string(),
            uid: uid.to_string(),
            action: action.to_string(),
            options,
        }
    }
}
