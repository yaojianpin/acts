use crate::{
    ActError, ActRunAs, MessageState, Result, Workflow, package::ActPackageCatalog,
    scheduler::NodeData, store::data,
};
use serde::{Deserialize, Serialize};

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
    pub run_as: ActRunAs,
    pub resources: String,
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
    pub pid: String,
    pub nid: String,
    pub r#type: String,
    pub state: String,
    pub data: String,
    pub start_time: i64,
    pub end_time: i64,
    pub timestamp: i64,
}

/// One node of a run's graph, as `proc:graph` answers it: a model declaration
/// node, or one execution instance of it.
///
/// `nid` is the model node id — the key a [`crate::Message`] carries as its
/// `nid` and the read model's rows carry as theirs. `tid` is the execution
/// instance — the message's `tid` — so a client folds the message stream (and
/// the task rows) onto the graph by instance, not just by model node. A
/// declaration the flow has not reached appears once with no `tid` and no
/// `state`; a node instantiated more than once (a fan-out item, a `while`
/// turn) appears once per execution.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphNode {
    /// The model node id (`Message.nid`).
    pub nid: String,
    /// The execution instance id (`Message.tid`), or `None` for a node the
    /// flow has not reached.
    #[serde(default)]
    pub tid: Option<String>,
    /// The node's display name.
    pub name: String,
    /// The node's level: `workflow`, `branch` or `step` — the read model's
    /// own word, the one `Workflow::tree_output` and the task rows use — plus
    /// `block`, the kernel's own virtual container for a container package's
    /// inline acts (`<step>#acts` and friends). A `block` is not a declared
    /// level — the declaration declares no such node — so the tree display
    /// folds it into its parent step, while the node itself stays here: the
    /// messages and rows name it. A step carrying `uses` is still a `step`
    /// here; its *message* is the act payload (`Message.r#type == "act"`).
    pub kind: String,
    /// The package the node declares (`uses`), when it declares one. A
    /// dynamic execution the model does not declare (a pushed act, a join
    /// request) carries the package its own durable fact names.
    pub uses: Option<String>,
    /// The enclosing model node: a branch's step, a step's branch, a
    /// handler's step, a step's timeout branch, or the execution a dynamic
    /// node was pushed under. `None` for the workflow root.
    pub parent: Option<String>,
    /// The enclosing execution's `tid`, or `None` when the parent itself has
    /// no execution. Together with `parent` this identifies the parent
    /// instance when one model node appears several times.
    #[serde(default)]
    pub parent_tid: Option<String>,
    /// Pre-order index through the emitted nodes.
    pub order: usize,
    /// The step's declared `next`, when it names one. The declaration chain
    /// itself is the order of a parent's `steps`.
    pub next: Option<String>,
    /// This execution's state, in the read model's vocabulary (`ready|running|
    /// interrupted|completed|submitted|backed|cancelled|error|aborted|skipped|
    /// removed`), or `None` when the flow has not reached the node — it has no
    /// execution, or its execution is still the seeded `Created` one. Each
    /// execution instance answers its own state, so a fan-out's completed item
    /// and another item's skipped withdrawal are separate nodes.
    pub state: Option<String>,
    /// The generation `state` describes (a fan-out item or a `while` turn is
    /// one generation of the node). `None` while the node has no state.
    pub generation: Option<u64>,
}

/// The run's graph (`proc:graph`): the durable (materialized) model expanded
/// over the aggregate's executions. Every declared node appears — executed or
/// not, in declaration order — once per execution the run admitted, and every
/// execution the model does not declare (a client's pushed act, the delegated
/// join's request) appears under its creating execution. The model is the copy
/// the import seeds, so the container packages' inline acts already appear as
/// their kernel children (`<step>#acts`, `<step>#item`, `<step>#act<N>`) and
/// the ids match the messages the run announces; the kernel's own branch for
/// them is kinded `block`, which the tree display folds into its parent step —
/// the declaration declares no such branch.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcessGraph {
    pub pid: String,
    /// The model the run was started from.
    pub mid: String,
    pub nodes: Vec<GraphNode>,
}

impl ProcessGraph {
    /// The graph as an indented tree, one node per line, in declaration
    /// order — `{kind} id:{nid} name={name}  uses={uses} state={state}
    /// next={next}`, with `nil` for an absent fact. A fan-out node appears
    /// once per item (the same `id` repeated), and a container's virtual
    /// `block` renders no line of its own: its acts splice into its parent
    /// step's children.
    pub fn tree_output(&self) -> String {
        let mut output = String::new();
        let mut ancestors = Vec::new();
        for root in self.nodes.iter().filter(|node| node.parent.is_none()) {
            self.write_node(root, &mut ancestors, true, true, &mut output);
        }
        output
    }

    fn write_node(
        &self,
        node: &GraphNode,
        ancestors: &mut Vec<bool>,
        is_last: bool,
        is_root: bool,
        output: &mut String,
    ) {
        if !is_root {
            for ancestor_is_last in ancestors.iter() {
                output.push_str(if *ancestor_is_last { "    " } else { "│   " });
            }
            output.push_str(if is_last { "└── " } else { "├── " });
        }
        let fact = |value: &Option<String>| value.clone().unwrap_or_else(|| "nil".to_string());
        output.push_str(&format!(
            "{} id:{} name={}  uses={} state={} next={}\n",
            node.kind,
            node.nid,
            node.name,
            fact(&node.uses),
            fact(&node.state),
            fact(&node.next),
        ));
        // A virtual block renders no line, so its visible children take its
        // place in the parent's list; the connector of every child is then
        // decided by the flattened list, not by a hidden node's position.
        let children = self.visible_children(node);
        let len = children.len();
        for (index, child) in children.iter().enumerate() {
            if !is_root {
                ancestors.push(is_last);
            }
            self.write_node(child, ancestors, index + 1 == len, false, output);
            if !is_root {
                ancestors.pop();
            }
        }
    }

    /// The node's direct children by execution identity: an executed node's
    /// children are the executions it admitted; a declaration with no
    /// execution owns the placeholders sharing its enclosing execution.
    fn children(&self, node: &GraphNode) -> Vec<&GraphNode> {
        match node.tid.as_deref() {
            Some(tid) => self
                .nodes
                .iter()
                .filter(|child| child.parent_tid.as_deref() == Some(tid))
                .collect(),
            None => self
                .nodes
                .iter()
                .filter(|child| {
                    child.tid.is_none()
                        && child.parent_tid == node.parent_tid
                        && child.parent.as_deref() == Some(node.nid.as_str())
                })
                .collect(),
        }
    }

    /// The children a tree line actually shows: every virtual `block` is
    /// replaced by its own visible children, recursively.
    fn visible_children(&self, node: &GraphNode) -> Vec<&GraphNode> {
        let mut visible = Vec::new();
        self.collect_visible_children(node, &mut visible);
        visible
    }

    fn collect_visible_children<'a>(&'a self, node: &GraphNode, visible: &mut Vec<&'a GraphNode>) {
        for child in self.children(node) {
            if child.kind == "block" {
                self.collect_visible_children(child, visible);
            } else {
                visible.push(child);
            }
        }
    }
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
    pub view: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MessageInfo {
    /// delivery id — the storage key of one (message × channel) delivery row
    pub id: String,

    /// the workflow message id shared by every channel delivery of the event
    pub msg_id: String,

    /// the channel this delivery row belongs to
    pub chan_id: String,

    pub tid: String,
    pub name: String,
    pub state: MessageState,
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
    pub ver: String,

    /// trigger kind, see [`crate::Trigger`]
    pub kind: String,
    /// default start inputs (trigger params) as json text
    pub params: String,
    /// kind=`schedule`: cron expression
    pub schedule: Option<String>,

    /// kind=`schedule`: last run time in millis
    pub last_run: i64,
    /// kind=`schedule`: next run time in millis
    pub next_run: i64,

    pub create_time: i64,
    pub timestamp: i64,
}

impl From<&data::Package> for PackageInfo {
    fn from(m: &data::Package) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            desc: m.desc.clone(),
            icon: m.icon.clone(),
            doc: m.doc.clone(),
            version: m.version.clone(),
            schema: m.schema.clone(),
            options: m.options.clone(),
            run_as: m.run_as,
            resources: m.resources.clone(),
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
                m.set_ver(&self.ver);
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
            desc: m.desc,
            ver: m.ver,
            size: m.size,
            create_time: m.create_time,
            update_time: m.update_time,
            data: m.data,
            view: m.view,
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
            // scope vars live in the paired vars row — executors join it
            // (`Store::vars`) when building manager views from durable rows
            data: String::new(),
            start_time: t.start_time,
            end_time: t.end_time,
            timestamp: t.timestamp,
        }
    }
}
impl From<&data::Task> for TaskInfo {
    fn from(t: &data::Task) -> Self {
        t.clone().into()
    }
}

impl MessageInfo {
    /// Build the manager view of one delivery: delivery state/identity joined
    /// with the payload and workflow context of its canonical message.
    pub fn from_delivery(delivery: &data::Delivery, message: &data::Message) -> Self {
        Self {
            // delivery identity — the target of ack/retry/clear/redo
            id: delivery.id.clone(),
            msg_id: delivery.msg_id.clone(),
            chan_id: delivery.chan_id.clone(),
            // message payload and workflow context
            tid: message.tid.clone(),
            name: message.name.clone(),
            state: message.state,
            r#type: message.r#type.clone(),
            pid: message.pid.clone(),
            nid: message.nid.clone(),
            inputs: message.inputs.clone(),
            outputs: message.outputs.clone(),
            uses: message.uses.clone(),
            timestamp: message.timestamp,
            // delivery state
            create_time: delivery.create_time,
            update_time: delivery.update_time,
            retry_times: delivery.retry_times,
            status: delivery.status.to_string(),
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
            ver: m.ver.clone(),
            kind: m.kind.clone(),
            params: m.params.clone(),
            schedule: m.schedule.clone(),
            last_run: m.last_run,
            next_run: m.next_run,
        }
    }
}

impl From<EventInfo> for serde_json::Value {
    fn from(val: EventInfo) -> Self {
        serde_json::to_value(val).unwrap()
    }
}
