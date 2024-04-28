use crate::{sch, store::data, ActError, Result, Workflow};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PackageInfo {
    pub id: String,
    pub name: String,
    pub size: u32,
    pub create_time: i64,
    pub update_time: i64,
    pub timestamp: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcInfo {
    pub id: String,
    pub name: String,
    pub mid: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub timestamp: i64,
    pub tasks: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskInfo {
    pub id: String,
    pub prev: Option<String>,
    pub name: String,
    pub proc_id: String,
    pub node_id: String,
    pub r#type: String,
    pub state: String,
    pub data: String,
    pub start_time: i64,
    pub end_time: i64,
    pub timestamp: i64,
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

impl From<&data::Package> for PackageInfo {
    fn from(m: &data::Package) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            size: m.size,
            timestamp: m.timestamp,
            create_time: m.create_time,
            update_time: m.update_time,
        }
    }
}

impl ModelInfo {
    pub fn workflow(&self) -> Result<Workflow> {
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

impl From<data::Model> for ModelInfo {
    fn from(m: data::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            ver: m.ver,
            size: m.size,
            time: m.time,
            model: m.data,
        }
    }
}

impl From<&data::Proc> for ProcInfo {
    fn from(p: &data::Proc) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            mid: p.mid.clone(),
            state: p.state.clone(),
            start_time: p.start_time,
            end_time: p.end_time,
            timestamp: p.timestamp,
            tasks: "".to_string(),
        }
    }
}

impl From<data::Task> for TaskInfo {
    fn from(t: data::Task) -> Self {
        Self {
            id: t.task_id,
            prev: t.prev,
            name: t.name,
            proc_id: t.proc_id,
            node_id: t.node_id,
            r#type: t.kind,
            state: t.state,
            data: t.data,
            start_time: t.start_time,
            end_time: t.end_time,
            timestamp: t.timestamp,
        }
    }
}

impl Into<serde_json::Value> for PackageInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "id": self.id,
            "name": self.name,
            "size": self.size,
            "timestamp": self.timestamp,
            "create_time": self.create_time,
            "update_time": self.update_time,
        })
    }
}

impl Into<serde_json::Value> for TaskInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "id": self.id,
            "name": self.name,
            "proc_id": self.proc_id,
            "node_id": self.node_id,
            "type": self.r#type,
            "state": self.state,
            "data": self.data,
            "start_time": self.start_time,
            "end_time": self.end_time,
            "timestamp": self.timestamp,
        })
    }
}

impl Into<serde_json::Value> for ProcInfo {
    fn into(self) -> serde_json::Value {
        json!({
            "id": self.id,
            "mid": self.mid,
            "name": self.name,
            "state": self.state,
            "start_time": self.start_time,
            "end_time": self.end_time,
            "timestamp": self.timestamp,
            "tasks": self.tasks,
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
        })
    }
}

impl From<&Arc<sch::Task>> for TaskInfo {
    fn from(t: &Arc<sch::Task>) -> Self {
        Self {
            id: t.id.clone(),
            prev: t.prev(),
            name: t.node.content.name(),
            proc_id: t.proc_id.clone(),
            node_id: t.node.id().to_string(),
            r#type: t.node.kind().into(),
            state: t.state().into(),
            data: t.data().to_string(),
            start_time: t.start_time(),
            end_time: t.end_time(),
            timestamp: t.timestamp,
        }
    }
}
