use crate::{
    sch::{self, Act},
    store::{self, Model, Proc, Task},
    ActError, ActResult, Workflow,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

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
pub struct ActInfo {
    pub pid: String,
    pub tid: String,
    pub aid: String,
    pub kind: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub ver: u32,
    pub size: u32,
    pub time: i64,
    pub model: String,
    pub topic: String,
}

impl ModelInfo {
    pub fn workflow(&self) -> ActResult<Workflow> {
        let m = serde_yaml::from_str::<Workflow>(&self.model);
        match m {
            Ok(mut m) => {
                m.set_ver(self.ver);
                Ok(m)
            }
            Err(err) => Err(ActError::Convert(err.to_string())),
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
            topic: m.topic,
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

impl Into<serde_json::Value> for ActInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "pid": self.pid,
            "tid": self.tid,
            "aid": self.aid,
            "kind": self.kind,
            "state": self.state,
            "start_time": self.start_time,
            "end_time": self.end_time,
        })
    }
}

impl Into<serde_json::Value> for TaskInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "pid": self.pid,
            "tid": self.tid,
            "nid": self.nid,
            "kind": self.kind,
            "state": self.state,
            "start_time": self.start_time,
            "end_time": self.end_time,
        })
    }
}

impl Into<serde_json::Value> for ProcInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "pid": self.pid,
            "mid": self.mid,
            "name": self.name,
            "state": self.state,
            "start_time": self.start_time,
            "end_time": self.end_time,
        })
    }
}

impl Into<serde_json::Value> for ModelInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "id": self.id,
            "name": self.name,
            "ver": self.ver,
            "size": self.size,
            "time": self.time,
            "model": self.model,
            "topic": self.topic,
        })
    }
}

impl From<&Arc<Act>> for ActInfo {
    fn from(act: &Arc<Act>) -> Self {
        Self {
            pid: act.pid.clone(),
            tid: act.tid.clone(),
            aid: act.id.clone(),
            kind: act.kind.to_string(),
            state: act.state().into(),
            start_time: act.start_time(),
            end_time: act.end_time(),
        }
    }
}

impl From<store::Act> for ActInfo {
    fn from(act: store::Act) -> Self {
        Self {
            pid: act.pid.clone(),
            tid: act.tid.clone(),
            aid: act.id.clone(),
            kind: act.kind,
            state: act.state,
            start_time: act.start_time,
            end_time: act.end_time,
        }
    }
}

impl From<&Arc<sch::Task>> for TaskInfo {
    fn from(t: &Arc<sch::Task>) -> Self {
        Self {
            pid: t.pid.clone(),
            tid: t.tid.clone(),
            nid: t.nid(),
            kind: t.node.kind().into(),
            state: t.state().into(),
            start_time: t.start_time(),
            end_time: t.end_time(),
        }
    }
}
