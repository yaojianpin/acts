use serde::{Deserialize, Serialize};

use crate::{
    store::{Message, Model, Proc, Task},
    ActError, ActResult, Workflow,
};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcInfo {
    pub pid: String,
    pub name: String,
    pub mid: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    // pub vars: Vars,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskInfo {
    pub pid: String,
    pub tid: String,
    pub nid: String,
    pub kind: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MessageInfo {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub create_time: i64,
    pub update_time: i64,
    pub uid: Option<String>,
    pub state: u8,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub ver: u32,
    pub size: u32,
    pub time: i64,
    pub model: String,
}

impl ModelInfo {
    pub fn workflow(&self) -> ActResult<Workflow> {
        let m = serde_yaml::from_str::<Workflow>(&self.model);
        match m {
            Ok(mut m) => {
                m.set_ver(self.ver);
                Ok(m)
            }
            Err(err) => Err(ActError::ConvertError(err.to_string())),
        }
    }
}

impl From<Model> for ModelInfo {
    fn from(m: Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            ver: m.ver,
            size: m.size,
            time: m.time,

            model: m.model,
        }
    }
}

impl From<&Proc> for ProcInfo {
    fn from(p: &Proc) -> Self {
        let model = Workflow::from_str(&p.model).unwrap();
        Self {
            pid: p.pid.clone(),
            name: model.name,
            mid: model.id,
            state: p.state.clone().into(),
            start_time: p.start_time,
            end_time: p.end_time,
        }
    }
}

impl From<Task> for TaskInfo {
    fn from(t: Task) -> Self {
        Self {
            pid: t.pid,
            tid: t.tid,
            nid: t.nid,
            kind: t.kind,
            state: t.state.into(),
            start_time: t.start_time,
            end_time: t.end_time,
        }
    }
}

impl From<Message> for MessageInfo {
    fn from(m: Message) -> Self {
        Self {
            id: m.id,
            pid: m.pid,
            tid: m.tid,
            state: m.state.into(),
            create_time: m.create_time,
            update_time: m.update_time,
            uid: if !m.uid.is_empty() { Some(m.uid) } else { None },
        }
    }
}
