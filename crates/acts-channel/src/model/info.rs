use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Deserialize, Serialize)]
pub struct PageData<T> {
    pub count: usize,
    pub page_size: usize,
    pub page_num: usize,
    pub page_count: usize,
    pub rows: Vec<T>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PackageInfo {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub icon: String,
    pub doc: String,
    pub version: String,
    pub schema: String,
    pub options: Option<String>,
    pub run_as: String,
    pub resources: String,
    pub catalog: String,

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

/// One node of a run's graph, as `proc:graph` answers it. The declaration
/// fields (`nid`, `name`, `kind`, `uses`, `params`, `vars`) come from the
/// materialized model; `tid`/`state`/`generation` name one execution instance,
/// and are empty for a declaration the flow has not reached.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphNode {
    /// The model node id (`Message.nid`).
    pub nid: String,
    /// The execution instance id (`Message.tid`), or `None` when the flow has
    /// not reached the node.
    #[serde(default)]
    pub tid: Option<String>,
    pub name: String,
    /// `workflow`, `branch`, `step`, or the kernel's virtual `block`.
    pub kind: String,
    /// The package the node declares (`uses`).
    pub uses: Option<String>,
    /// The enclosing model node; `None` for the workflow root.
    pub parent: Option<String>,
    /// The enclosing execution's `tid`; `None` when the parent has none.
    #[serde(default)]
    pub parent_tid: Option<String>,
    /// Pre-order index through the emitted nodes.
    pub order: usize,
    /// The step's declared `next`.
    pub next: Option<String>,
    /// The execution's state in the read model's vocabulary, or `None` while
    /// the flow has not reached the node.
    pub state: Option<String>,
    /// The generation `state` describes.
    pub generation: Option<u64>,
    /// The step's declared params; `null` when it declares none.
    #[serde(default)]
    pub params: JsonValue,
    /// The step's or branch's declared vars, in declaration order.
    #[serde(default)]
    pub vars: Vec<GraphVar>,
}

/// One declared step variable on the graph. The server writes the model's
/// variant shape, whose name/value pair is all a graph client reads.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphVar {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: JsonValue,
}

/// A run's graph (`proc:graph`): the model's declaration expanded over the
/// aggregate's execution instances, in declaration order.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcessGraph {
    pub pid: String,
    pub mid: String,
    pub nodes: Vec<GraphNode>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub ver: String,
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
    pub state: String,
    pub r#type: String,
    pub pid: String,
    pub nid: String,
    pub inputs: String,
    pub outputs: String,
    pub create_time: i64,
    pub update_time: i64,
    pub retry_times: i32,
    pub status: String,
    pub timestamp: i64,
    pub uses: Option<String>,
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

impl From<EventInfo> for serde_json::Value {
    fn from(val: EventInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{EventInfo, MessageInfo, ModelInfo, PackageInfo, PageData, ProcInfo, TaskInfo};
    use serde_json::json;

    fn package_info() -> PackageInfo {
        PackageInfo {
            id: "pkg".to_string(),
            name: "package".to_string(),
            desc: "a package".to_string(),
            icon: "icon.svg".to_string(),
            doc: "docs".to_string(),
            version: "1.0".to_string(),
            schema: "{}".to_string(),
            options: Some("{}".to_string()),
            run_as: "user".to_string(),
            resources: "[]".to_string(),
            catalog: "app".to_string(),
            create_time: 1,
            update_time: 2,
            timestamp: 3,
        }
    }

    fn task_info() -> TaskInfo {
        TaskInfo {
            id: "task".to_string(),
            prev: Some("prev".to_string()),
            name: "act".to_string(),
            pid: "proc".to_string(),
            nid: "node".to_string(),
            r#type: "act".to_string(),
            state: "created".to_string(),
            data: "{}".to_string(),
            start_time: 1,
            end_time: 2,
            timestamp: 3,
        }
    }

    fn proc_info() -> ProcInfo {
        ProcInfo {
            id: "proc".to_string(),
            name: "workflow".to_string(),
            mid: "model".to_string(),
            state: "running".to_string(),
            start_time: 1,
            end_time: 2,
            timestamp: 3,
            tasks: vec![task_info()],
        }
    }

    fn model_info() -> ModelInfo {
        ModelInfo {
            id: "model".to_string(),
            name: "workflow".to_string(),
            desc: "a model".to_string(),
            ver: "1.0".to_string(),
            size: 10,
            create_time: 1,
            update_time: 2,
            data: "{}".to_string(),
        }
    }

    fn message_info() -> MessageInfo {
        MessageInfo {
            id: "msg".to_string(),
            tid: "task".to_string(),
            name: "act".to_string(),
            state: "completed".to_string(),
            r#type: "msg".to_string(),
            pid: "proc".to_string(),
            nid: "node".to_string(),
            inputs: "{}".to_string(),
            outputs: "{}".to_string(),
            create_time: 1,
            update_time: 2,
            retry_times: 0,
            status: "acked".to_string(),
            timestamp: 3,
            uses: Some("uses".to_string()),
        }
    }

    fn event_info() -> EventInfo {
        EventInfo {
            id: "evt".to_string(),
            name: "trigger".to_string(),
            mid: "model".to_string(),
            ver: 1,
            uses: "uses".to_string(),
            params: "{}".to_string(),
            create_time: 1,
            timestamp: 2,
        }
    }

    #[test]
    fn package_info_roundtrips_and_converts() {
        let info = package_info();
        let value: serde_json::Value = info.clone().into();
        assert_eq!(value["id"], json!("pkg"));
        assert_eq!(value["options"], json!("{}"));

        let text = serde_json::to_string(&info).unwrap();
        let back: PackageInfo = serde_json::from_str(&text).unwrap();
        assert_eq!(back.name, "package");
        assert_eq!(back.create_time, 1);
        assert_eq!(back.timestamp, 3);
    }

    #[test]
    fn task_info_roundtrips_and_converts() {
        let info = task_info();
        let value: serde_json::Value = info.clone().into();
        assert_eq!(value["id"], json!("task"));
        assert_eq!(value["prev"], json!("prev"));
        assert_eq!(value["type"], json!("act"));

        let back: TaskInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back.prev.as_deref(), Some("prev"));
        assert_eq!(back.state, "created");
    }

    #[test]
    fn proc_info_roundtrips_with_its_tasks() {
        let info = proc_info();
        let value: serde_json::Value = info.clone().into();
        assert_eq!(value["id"], json!("proc"));
        assert_eq!(value["tasks"][0]["id"], json!("task"));

        let back: ProcInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back.tasks.len(), 1);
        assert_eq!(back.tasks[0].name, "act");
    }

    #[test]
    fn model_info_roundtrips_and_converts() {
        let info = model_info();
        let value: serde_json::Value = info.clone().into();
        assert_eq!(value["id"], json!("model"));
        assert_eq!(value["size"], json!(10));

        let back: ModelInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back.ver, "1.0");
        assert_eq!(back.update_time, 2);
    }

    #[test]
    fn message_info_roundtrips_and_converts() {
        let info = message_info();
        let value: serde_json::Value = info.clone().into();
        assert_eq!(value["id"], json!("msg"));
        assert_eq!(value["uses"], json!("uses"));

        let back: MessageInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back.status, "acked");
        assert_eq!(back.uses.as_deref(), Some("uses"));

        // `uses` is optional on the wire
        let mut value: serde_json::Value = info.into();
        value.as_object_mut().unwrap().remove("uses");
        let back: MessageInfo = serde_json::from_value(value).unwrap();
        assert!(back.uses.is_none());
    }

    #[test]
    fn event_info_roundtrips_and_converts() {
        let info = event_info();
        let value: serde_json::Value = info.clone().into();
        assert_eq!(value["id"], json!("evt"));
        assert_eq!(value["ver"], json!(1));

        let back: EventInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back.name, "trigger");
        assert_eq!(back.uses, "uses");
    }

    #[test]
    fn page_data_carries_the_page_shape() {
        let page = PageData {
            count: 1,
            page_size: 10,
            page_num: 2,
            page_count: 3,
            rows: vec![proc_info()],
        };
        let value = serde_json::to_value(&page).unwrap();
        assert_eq!(value["count"], json!(1));
        assert_eq!(value["page_size"], json!(10));
        assert_eq!(value["page_num"], json!(2));
        assert_eq!(value["page_count"], json!(3));
        assert_eq!(value["rows"][0]["id"], json!("proc"));
        let back: PageData<ProcInfo> = serde_json::from_value(value).unwrap();
        assert_eq!(back.count, 1);
        assert_eq!(back.rows.len(), 1);
    }
}
