use crate::{
    ActError, ActRunAs, MessageState, Result, Workflow,
    package::ActPackageCatalog,
    scheduler::{self, NodeData},
    store::data,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PackageInfo {
    pub id: String,

    pub desc: String,
    pub icon: String,
    pub doc: String,
    pub version: String,
    pub schema: String,
    pub run_as: ActRunAs,
    pub groups: String,
    pub catalog: ActPackageCatalog,

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
    pub tag: String,
    pub key: String,
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
    pub ver: i32,
    pub size: i32,
    pub create_time: i64,
    pub update_time: i64,
    pub data: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MessageInfo {
    pub id: String,
    pub tid: String,
    pub name: String,
    pub state: MessageState,
    pub r#type: String,
    pub pid: String,
    pub nid: String,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EventInfo {
    pub id: String,
    pub name: String,
    pub mid: String,
    pub ver: i32,

    pub uses: String,
    pub params: String,

    pub create_time: i64,
    pub timestamp: i64,
}

impl From<&data::Package> for PackageInfo {
    fn from(m: &data::Package) -> Self {
        Self {
            id: m.id.clone(),
            desc: m.desc.clone(),
            icon: m.icon.clone(),
            doc: m.doc.clone(),
            version: m.version.clone(),
            schema: m.schema.clone(),
            run_as: m.run_as,
            groups: m.groups.clone(),
            catalog: m.catalog,

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
            create_time: m.create_time,
            update_time: m.update_time,
            data: m.data,
        }
    }
}

impl From<&data::Model> for ModelInfo {
    fn from(m: &data::Model) -> Self {
        m.clone().into()
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
            key: node_data.content.key(),
            tag: node_data.content.tag(),
        }
    }
}

impl From<&data::Task> for TaskInfo {
    fn from(t: &data::Task) -> Self {
        t.clone().into()
    }
}

impl From<&Arc<scheduler::Task>> for TaskInfo {
    fn from(t: &Arc<scheduler::Task>) -> Self {
        Self {
            id: t.id.clone(),
            prev: t.prev(),
            name: t.node().content.name(),
            pid: t.pid.clone(),
            nid: t.node().id().to_string(),
            r#type: t.node().kind().to_string(),
            state: t.state().into(),
            data: t.data().to_string(),
            start_time: t.start_time(),
            end_time: t.end_time(),
            timestamp: t.timestamp,
            tag: t.node().tag(),
            key: t.node().key(),
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
            nid: m.nid.clone(),
            timestamp: m.timestamp,
            create_time: m.create_time,
            update_time: m.update_time,
            state: m.state,
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

impl From<PackageInfo> for serde_json::Value {
    fn from(val: PackageInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}

impl From<TaskInfo> for serde_json::Value {
    fn from(val: TaskInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}

impl From<ProcInfo> for serde_json::Value {
    fn from(val: ProcInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}

impl From<ModelInfo> for serde_json::Value {
    fn from(val: ModelInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}

impl From<MessageInfo> for serde_json::Value {
    fn from(val: MessageInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}

impl From<&data::Event> for EventInfo {
    fn from(m: &data::Event) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            timestamp: m.timestamp,
            create_time: m.create_time,
            mid: m.mid.clone(),
            ver: m.ver,
            uses: m.uses.clone(),
            params: m.params.clone(),
        }
    }
}

impl From<EventInfo> for serde_json::Value {
    fn from(val: EventInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}
