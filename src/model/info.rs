use crate::{
    sch::{self, NodeData},
    store::data,
    ActError, Result, Workflow,
};
use serde::{Deserialize, Serialize};
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
    pub tasks: Vec<TaskInfo>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskInfo {
    pub id: String,
    pub prev: Option<String>,
    pub name: String,
    pub pid: String,
    pub nid: String,
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
    pub data: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MessageInfo {
    pub id: String,
    pub tid: String,
    pub name: String,
    pub state: String,
    pub r#type: String,
    pub pid: String,
    pub key: String,
    pub inputs: String,
    pub outputs: String,
    pub tag: String,
    pub create_time: i64,
    pub update_time: i64,
    pub retry_times: i32,
    pub status: String,
    pub timestamp: i64,
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
        let m = serde_yaml::from_str::<Workflow>(&self.data);
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
            data: m.data,
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
            tasks: Vec::new(),
        }
    }
}

impl From<data::Task> for TaskInfo {
    fn from(t: data::Task) -> Self {
        let node_data: NodeData = serde_json::from_str(&t.node_data).unwrap();
        Self {
            id: t.tid,
            prev: t.prev,
            name: t.name,
            pid: t.pid,
            nid: node_data.id,
            r#type: t.kind,
            state: t.state,
            data: t.data,
            start_time: t.start_time,
            end_time: t.end_time,
            timestamp: t.timestamp,
        }
    }
}

impl From<&Arc<sch::Task>> for TaskInfo {
    fn from(t: &Arc<sch::Task>) -> Self {
        Self {
            id: t.id.clone(),
            prev: t.prev(),
            name: t.node().content.name(),
            pid: t.pid.clone(),
            nid: t.node().id().to_string(),
            r#type: t.node().kind().into(),
            state: t.state().into(),
            data: t.data().to_string(),
            start_time: t.start_time(),
            end_time: t.end_time(),
            timestamp: t.timestamp,
        }
    }
}

impl From<&data::Message> for MessageInfo {
    fn from(m: &data::Message) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            pid: m.pid.clone(),
            tid: m.tid.clone(),
            timestamp: m.timestamp,
            create_time: m.create_time,
            update_time: m.update_time,
            state: m.state.clone(),
            r#type: m.r#type.clone(),
            key: m.key.clone(),
            tag: m.tag.clone(),

            inputs: m.inputs.clone(),
            outputs: m.outputs.clone(),
            retry_times: m.retry_times,
            status: m.status.to_string(),
        }
    }
}

impl Into<serde_json::Value> for PackageInfo {
    fn into(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

impl Into<serde_json::Value> for TaskInfo {
    fn into(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

impl Into<serde_json::Value> for ProcInfo {
    fn into(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

impl Into<serde_json::Value> for ModelInfo {
    fn into(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

impl Into<serde_json::Value> for MessageInfo {
    fn into(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}
