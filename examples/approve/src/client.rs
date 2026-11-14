//! The interactive demo client: every human act the engine announces is put
//! to the terminal as an arrow-key menu (complete / add approver / remove
//! approver / back / submit / remind / transfer / delegate / cancel / graph /
//! exit); the client fills in `pid`, `tid` and the acting user, runs the
//! choice and waits for the next act, until the workflow completes. `back`
//! and `cancel` ask for a predecessor of the current node (never the act's
//! own node or a subtask), and `add approver` lists the tasks under the
//! current node and lets the selected task's shape pick the kernel's push
//! spelling. The answers that need no user live in `sys.rs`. A `--script`
//! replays one recorded choice per prompt; a `--record` session writes exactly
//! that file, so a misbehaving run hands over its own repro.

use acts::{Engine, Executor, Message, MessageState, Principal, Result, UserPolicy, Vars};
use dialoguer::{Input, Select, theme::ColorfulTheme};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The demo actors' allowed actions: every human in the directory may
/// decide, sign, urge, skip, fail, abort and cancel in this demo.
const ACTOR_ACTIONS: [&str; 10] = [
    "act:complete",
    "act:submit",
    "act:back",
    "act:push",
    "act:remove",
    "act:remind",
    "act:cancel",
    "act:skip",
    "act:error",
    "act:abort",
];

/// One decision the menu offers on an act.
#[derive(Clone, Copy)]
enum Menu {
    Complete,
    AddApprover,
    RemoveApprover,
    Back,
    Submit,
    Remind,
    Transfer,
    Delegate,
    Skip,
    Error,
    Abort,
    Leave,
    Graph,
    Cancel,
}

/// A prompt that can no longer be answered stops the run loudly: a drifted
/// script is the bug report itself, never something to replay loosely.
fn quit(message: &str) -> ! {
    eprintln!("   {message}");
    std::process::exit(1);
}

/// Where the menu choices come from.
///
/// * `Terminal` — the interactive prompt: every enumerated choice is an
///   arrow-key select rendered at the bottom of the transcript (redrawn in
///   place until Enter), free text is a line editor. With `--record`, one
///   line per answer is appended to the file, in the exact format
///   `--script` consumes, so an interactive session becomes its own repro
///   script.
/// * `Script` — one line per prompt (`--script` file or stdin): a select's
///   line is the item text, its 1-based position, or a stable key (a history
///   menu prints the task id but keys its entries by node name), a text
///   prompt's line is the text itself, a blank line takes the default. The
///   line is echoed, so the transcript reads like a typed session.
pub enum Prompter {
    Terminal(Option<std::fs::File>),
    Script {
        lines: Mutex<std::collections::VecDeque<String>>,
        number: AtomicUsize,
    },
}

impl Default for Prompter {
    fn default() -> Self {
        Self::Terminal(None)
    }
}

impl Prompter {
    /// A script of choices: `-` reads it from stdin, any other value from
    /// that file.
    pub fn script(source: &str) -> std::io::Result<Self> {
        let text = if source == "-" {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            text
        } else {
            std::fs::read_to_string(source)?
        };
        Ok(Self::Script {
            lines: Mutex::new(
                text.lines()
                    // A scenario file documents itself with `#` lines; every
                    // other line is one choice (a blank line is the prompt's
                    // default choice).
                    .filter(|line| !line.trim_start().starts_with('#'))
                    .map(|line| line.trim().to_string())
                    .collect(),
            ),
            number: AtomicUsize::new(0),
        })
    }

    /// The interactive terminal plus a recording file (`--record`).
    pub fn recording(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self::Terminal(Some(file)))
    }

    /// Append one answer to the recording: a select writes its stable key
    /// (the item text unless the caller gave a separate key), an input the
    /// typed line.
    fn record(record: &Option<std::fs::File>, answer: &str) {
        if let Some(record) = record {
            use std::io::Write;
            let mut record = record;
            let _ = writeln!(record, "{answer}");
        }
    }

    /// One choice among `items`; returns the picked index. `default` is the
    /// preselected entry, also taken when a script line is blank. `None`
    /// means the script ran out — the caller decides what that leaves to
    /// the engine.
    pub async fn select(&self, prompt: &str, items: &[&str], default: usize) -> Option<usize> {
        self.select_keyed(prompt, items, items, default).await
    }

    /// One choice whose display label and stable script key may differ: a
    /// history entry prints its task id, while `--record` writes the key, so
    /// the script can replay on another run. A script line matches the label,
    /// its 1-based position, or one key; `keys` has one entry per item.
    pub async fn select_keyed(
        &self,
        prompt: &str,
        items: &[&str],
        keys: &[&str],
        default: usize,
    ) -> Option<usize> {
        assert_eq!(items.len(), keys.len(), "every choice needs one key");
        match self {
            Self::Terminal(record) => {
                let prompt = prompt.to_string();
                let items: Vec<String> = items.iter().map(|item| item.to_string()).collect();
                let picked = tokio::task::spawn_blocking(move || {
                    Select::with_theme(&ColorfulTheme::default())
                        .with_prompt(&prompt)
                        .items(&items)
                        .default(default)
                        .interact()
                })
                .await
                .unwrap_or_else(|error| quit(&format!("the menu task failed: {error}")))
                .unwrap_or_else(|error| quit(&format!("the menu failed: {error}")));
                Self::record(record, keys[picked]);
                Some(picked)
            }
            Self::Script { lines, number } => {
                let line_no = number.fetch_add(1, Ordering::SeqCst) + 1;
                let Some(line) = lines.lock().unwrap().pop_front() else {
                    println!();
                    return None;
                };
                println!("{line}");
                let line = line.trim();
                if line.is_empty() {
                    return Some(default);
                }
                // An exact item text wins over a 1-based position: the demo's
                // user ids are numbers themselves.
                if let Some(index) = items.iter().position(|item| *item == line) {
                    return Some(index);
                }
                if let Ok(position) = line.parse::<usize>()
                    && (1..=items.len()).contains(&position)
                {
                    return Some(position - 1);
                }
                let matching: Vec<usize> = keys
                    .iter()
                    .enumerate()
                    .filter(|(_, key)| **key == line)
                    .map(|(index, _)| index)
                    .collect();
                match matching.as_slice() {
                    [index] => Some(*index),
                    [] => quit(&format!(
                        "script line {line_no} is '{line}': no such choice here — one of {items:?} (exact text, 1-based position or key)"
                    )),
                    _ => quit(&format!(
                        "script line {line_no} is '{line}': {} choices share that key — use the exact label or its 1-based position",
                        matching.len()
                    )),
                }
            }
        }
    }

    /// One free-text line (may be empty).
    pub async fn input(&self, prompt: &str) -> String {
        match self {
            Self::Terminal(record) => {
                let prompt = prompt.to_string();
                let line = tokio::task::spawn_blocking(move || {
                    Input::<String>::with_theme(&ColorfulTheme::default())
                        .with_prompt(&prompt)
                        .allow_empty(true)
                        .interact_text()
                })
                .await
                .unwrap_or_else(|error| quit(&format!("the input task failed: {error}")))
                .unwrap_or_else(|error| quit(&format!("the input failed: {error}")));
                Self::record(record, &line);
                line
            }
            Self::Script { lines, number } => {
                // The counter stays in step with the prompts, so a select's
                // error names the script line the user actually wrote.
                number.fetch_add(1, Ordering::SeqCst);
                let Some(line) = lines.lock().unwrap().pop_front() else {
                    println!();
                    return String::new();
                };
                println!("{line}");
                line.trim().to_string()
            }
        }
    }
}

/// Print an action refusal; `true` when it means the act settled elsewhere
/// (a withdrawn vote, a rewound act) and the menu should stop.
fn refused_settled(error: &acts::ActError, nid: &str) -> bool {
    let text = error.to_string();
    if text.contains("ActionNotAllowed") {
        println!("   the act {nid} already settled — left to the engine");
        return true;
    }
    println!("   refused: {text} — choose again");
    false
}

/// One execution a menu can offer: the task instance (`tid`) and the node it
/// runs (`nid`, `name`, `state`).
#[derive(Clone, Debug)]
struct HistoryEntry {
    nid: String,
    tid: String,
    name: String,
    state: String,
}

impl HistoryEntry {
    /// The node's display name; a history entry without a declared name gets
    /// its action key (or node id) at build time, so this is the final
    /// fallback for a row that has neither.
    fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.nid
        } else {
            &self.name
        }
    }

    /// The menu label: the node name and the task id, the two facts a user
    /// needs to pick one instance among several generations.
    fn label(&self) -> String {
        format!("{}  task={}", self.display_name(), self.tid)
    }

    /// The label with the instance's state, so a menu that asks *which* task
    /// to rewind or cancel tells a finished node from one the guard skipped.
    fn label_with_state(&self) -> String {
        format!(
            "{}  task={}  state={}",
            self.display_name(),
            self.tid,
            self.state
        )
    }
}

/// The graph's executions by their wire `tid`.
fn nodes_by_tid(graph: &acts::ProcessGraph) -> HashMap<&str, &acts::GraphNode> {
    let mut by_tid = HashMap::new();
    for node in &graph.nodes {
        if let Some(tid) = node.tid.as_deref() {
            by_tid.insert(tid, node);
        }
    }
    by_tid
}

/// One execution's entry: the node name (the action key for an unnamed act,
/// the name a user knows it by and a stable script key across runs) and the
/// state it announced. The graph is the source for an execution the read
/// model does not project as a task row — a pushed act has a graph node but
/// no `ProcInfo.tasks` entry.
fn history_entry(node: Option<&acts::GraphNode>, task: Option<&acts::TaskInfo>) -> HistoryEntry {
    let nid = task
        .map(|task| task.nid.clone())
        .or_else(|| node.map(|node| node.nid.clone()))
        .unwrap_or_default();
    let tid = task
        .map(|task| task.id.clone())
        .or_else(|| node.and_then(|node| node.tid.clone()))
        .unwrap_or_default();
    let name = task
        .map(|task| task.name.as_str())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            node.map(|node| node.name.as_str())
                .filter(|name| !name.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            node.and_then(|node| node.params.get("key"))
                .and_then(|key| key.as_str())
                .unwrap_or_default()
                .to_string()
        });
    let state = task
        .map(|task| task.state.clone())
        .or_else(|| node.and_then(|node| node.state.clone()))
        .unwrap_or_default();
    HistoryEntry {
        nid,
        tid,
        name,
        state,
    }
}

/// The history path from one execution back to the run's root: the task
/// predecessor chain when the row names one (the flow order), and the
/// execution's parent when it does not (a pushed act). An execution the read
/// model does not project as a task row — a pushed act — is read from its
/// graph node instead. The kernel's own groupings are walked through but not
/// listed — the virtual `block` branch, the fan-out item's `acts.core.block`
/// template and the `#cand` pool requests: none of them is a node a user acts
/// on.
fn history_path(
    graph: &acts::ProcessGraph,
    tasks: &[acts::TaskInfo],
    start: &str,
) -> Vec<HistoryEntry> {
    let by_tid = nodes_by_tid(graph);
    let mut path = Vec::new();
    let mut current = start.to_string();
    let mut seen = HashSet::new();
    while seen.insert(current.clone()) {
        let task = tasks.iter().find(|task| task.id == current);
        let node = by_tid.get(current.as_str()).copied();
        if task.is_none() && node.is_none() {
            break;
        }
        let nid = task
            .map(|task| task.nid.as_str())
            .or_else(|| node.map(|node| node.nid.as_str()))
            .unwrap_or_default();
        let kernel_grouping = node.is_some_and(|node| {
            node.kind == "block"
                || node.uses.as_deref() == Some("acts.core.block")
                || is_pool_request(nid)
        });
        if !kernel_grouping {
            path.push(history_entry(node, task));
        }
        // The row's `prev` is the node the flow came from; with several
        // generations of it, the latest one that started before this
        // execution is the one it inherited. A pushed act has no predecessor
        // row — its graph parent is the act it hangs under.
        let previous = task.and_then(|task| {
            let prev = task.prev.as_ref()?;
            tasks
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    &candidate.nid == prev
                        && candidate.id != current
                        && candidate.start_time > 0
                        && candidate.start_time <= task.start_time
                })
                .max_by_key(|(index, candidate)| (candidate.start_time, candidate.end_time, *index))
                .map(|(_, candidate)| candidate.id.clone())
        });
        let parent = node.and_then(|node| node.parent_tid.clone());
        match previous.or(parent) {
            Some(next) if next != current => current = next,
            _ => break,
        }
    }
    path
}

/// Whether a node id names one of the kernel's `#cand<k>` pool requests: the
/// container's own asking for its fan-out pool, never an act a user decides.
fn is_pool_request(nid: &str) -> bool {
    nid.rsplit('#')
        .next()
        .and_then(|tail| tail.strip_prefix("cand"))
        .is_some_and(|index| !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
        && nid.contains('#')
}

/// Whether a node id names a kernel-materialized wrapper: the virtual
/// `<step>#acts`/`<step>#in` branches, the fan-out item's `<step>#item`
/// template and its `<step>#act<N>` children. A declared step's own id has no
/// `#<wrapper>` tail, so a block step the model declares stays a real node.
fn is_wrapper_nid(nid: &str) -> bool {
    nid.rsplit_once('#').is_some_and(|(_, tail)| {
        matches!(tail, "acts" | "in" | "item")
            || tail
                .strip_prefix("act")
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The execution ancestry of one task: the instance itself and every
/// execution that admitted it, up to the workflow root.
fn task_ancestry(graph: &acts::ProcessGraph, tid: &str) -> HashSet<String> {
    let by_tid = nodes_by_tid(graph);
    let mut ancestry = HashSet::new();
    let mut current = Some(tid.to_string());
    while let Some(tid) = current {
        if !ancestry.insert(tid.clone()) {
            break;
        }
        current = by_tid
            .get(tid.as_str())
            .and_then(|node| node.parent_tid.clone());
    }
    ancestry
}

/// Whether a graph node is one of the kernel's own groupings rather than a
/// task a menu can offer: the workflow record, a virtual `block` branch, a
/// `#cand` pool request, and the packages whose node is a request or a
/// delivery rather than a decision (the candidate ask, the delegated join's
/// request, a notification).
fn is_kernel_grouping(node: &acts::GraphNode) -> bool {
    node.kind == "workflow"
        || node.kind == "block"
        || is_wrapper_nid(&node.nid)
        || is_pool_request(&node.nid)
        || matches!(
            node.uses.as_deref(),
            Some("acts.core.users") | Some("acts.core.decide") | Some("acts.core.msg")
        )
}

/// The flow predecessors of one act: the tasks the flow ran before the node
/// this act belongs to. The act's own node is excluded — the act itself and
/// every execution enclosing it (the fan-out vote's container) — as are the
/// kernel groupings and the workflow record, so `back`/`cancel` never offer
/// the current node, one of its subtasks, or a workflow message.
fn predecessor_tasks(
    graph: &acts::ProcessGraph,
    tasks: &[acts::TaskInfo],
    tid: &str,
) -> std::result::Result<Vec<HistoryEntry>, String> {
    let path = history_path(graph, tasks, tid);
    let Some(here) = path.first() else {
        return Err(format!("no task history for {tid}"));
    };
    let here = (here.display_name().to_string(), here.tid.clone());
    let ancestry = task_ancestry(graph, tid);
    let by_tid = nodes_by_tid(graph);
    let predecessors: Vec<HistoryEntry> = path
        .into_iter()
        .filter(|entry| !ancestry.contains(&entry.tid))
        .filter(|entry| {
            !by_tid
                .get(entry.tid.as_str())
                .is_some_and(|node| node.kind == "workflow")
        })
        .filter(|entry| {
            by_tid
                .get(entry.tid.as_str())
                .and_then(|node| node.uses.as_deref())
                != Some("acts.core.msg")
        })
        .collect();
    if predecessors.is_empty() {
        return Err(format!(
            "no predecessor task before {} (task {}) — this act starts the flow",
            here.0, here.1
        ));
    }
    Ok(predecessors)
}

/// The node one act belongs to: the nearest enclosing container step
/// (`parallel`/`sequence`/`block`), or the top-level step when the act is not
/// inside one. A fan-out vote and the container that admitted it are one
/// node; so is an approver pushed under either.
fn node_root(graph: &acts::ProcessGraph, tid: &str) -> Option<String> {
    let by_tid = nodes_by_tid(graph);
    let mut current = Some(tid.to_string());
    let mut top = tid.to_string();
    while let Some(tid) = current {
        let Some(node) = by_tid.get(tid.as_str()).copied() else {
            break;
        };
        if node.kind == "workflow" {
            break;
        }
        if is_container(node) {
            return Some(tid);
        }
        if node.kind == "step" {
            top = tid;
        }
        current = node.parent_tid.clone();
    }
    Some(top)
}

/// Whether one execution is a container step: its package fans child acts out
/// instead of waiting for one human decision.
fn is_container(node: &acts::GraphNode) -> bool {
    node.kind == "step"
        && !is_wrapper_nid(&node.nid)
        && matches!(
            node.uses.as_deref(),
            Some("acts.core.parallel") | Some("acts.core.sequence") | Some("acts.core.block")
        )
}

/// Every execution's children, in graph order.
fn children_by_graph(graph: &acts::ProcessGraph) -> HashMap<&str, Vec<&acts::GraphNode>> {
    let mut children: HashMap<&str, Vec<&acts::GraphNode>> = HashMap::new();
    for node in &graph.nodes {
        if let Some(parent) = node.parent_tid.as_deref() {
            children.entry(parent).or_default().push(node);
        }
    }
    for list in children.values_mut() {
        list.sort_by_key(|node| node.order);
    }
    children
}

/// The tasks of one node's execution subtree, in graph order: the node's own
/// task, the acts its container admitted (a fan-out's votes), and every act
/// pushed under it. The kernel's wrappers are walked through but not listed.
fn node_subtree(
    graph: &acts::ProcessGraph,
    tasks: &[acts::TaskInfo],
    root: &str,
) -> Vec<HistoryEntry> {
    let by_tid = nodes_by_tid(graph);
    let children = children_by_graph(graph);
    let mut entries = Vec::new();
    let mut pending = vec![root.to_string()];
    let mut seen = HashSet::new();
    while let Some(tid) = pending.pop() {
        if !seen.insert(tid.clone()) {
            continue;
        }
        if let Some(node) = by_tid.get(tid.as_str()).copied()
            && !is_kernel_grouping(node)
        {
            let task = tasks.iter().find(|task| task.id == tid);
            entries.push(history_entry(Some(node), task));
        }
        if let Some(children) = children.get(tid.as_str()) {
            for child in children.iter().rev() {
                if let Some(child) = child.tid.as_deref() {
                    pending.push(child.to_string());
                }
            }
        }
    }
    entries
}

/// The stable script keys for a task menu: the node name while it is unique
/// in the list, and `name#<index>` for a name that appears several times (a
/// fan-out's repeated act), so `--record` never writes a tid that the next
/// run cannot match.
fn history_keys(path: &[HistoryEntry]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for entry in path {
        *counts.entry(entry.display_name()).or_default() += 1;
    }
    path.iter()
        .enumerate()
        .map(|(index, entry)| {
            let name = entry.display_name();
            if counts.get(name).copied().unwrap_or(0) == 1 {
                name.to_string()
            } else {
                format!("{name}#{index}")
            }
        })
        .collect()
}

/// Print the menu's entries, one per line, with the state each announced.
fn print_entries(note: &str, entries: &[HistoryEntry]) {
    println!("   {note}");
    for entry in entries {
        println!("     {}", entry.label_with_state());
    }
}

#[derive(Default)]
pub struct Client {
    /// tid → the user whose decision the act currently waits on: the owner
    /// at creation, refreshed by every assignment notice (a transfer moves
    /// it, a delegation points it at the delegatee).
    deciders: Mutex<HashMap<String, String>>,
    /// authenticated executors per user id, built once at `bind` — the
    /// kernel attributes and authorizes every menu action by the subject.
    actors: Mutex<HashMap<String, Executor>>,
    /// One clerk processes the inbox: the demo answers one message at a
    /// time, so two answers never race in the same act's commit window.
    gate: tokio::sync::Mutex<()>,
    /// The tids whose menu is open right now. A menu runs detached, so a
    /// duplicate `Created` for one instance (a re-announce) must not open a
    /// second menu on the same act — two menus would race for one script's
    /// lines and one decider's answers.
    menus: Mutex<HashSet<String>>,
    /// The menu's input source: the terminal, or a script.
    prompter: Prompter,
    /// tid → the terminal state its act announced. A queued act can settle
    /// elsewhere (a `join: any` sibling passes and the join skips the rest);
    /// its menu then leaves the decision to the engine. The set fills before
    /// the clerk's gate, so a withdrawal lands while another act's menu is
    /// open.
    settled: Mutex<HashMap<String, String>>,
    /// The demo directory: the sys users plus `boss` — the offered choices
    /// of every user-picking prompt (approver, transfer, delegate, urge).
    users: Vec<String>,
    /// Raised when a script runs out of answers: the demo stops waiting for
    /// a run no client will decide any further.
    exhausted: Option<Arc<tokio::sync::Notify>>,
}

impl Client {
    pub fn new() -> Self {
        Client {
            ..Default::default()
        }
    }

    /// Drive the menu from a script of choices (`-` reads stdin).
    pub fn script(mut self, source: &str) -> std::io::Result<Self> {
        self.prompter = Prompter::script(source)?;
        Ok(self)
    }

    /// The interactive menu, plus a recording file (`--record`): one line
    /// per answer, in the exact format `script` consumes.
    pub fn recording(mut self, path: &std::path::Path) -> std::io::Result<Self> {
        self.prompter = Prompter::recording(path)?;
        Ok(self)
    }

    /// Raise `stop` when a script runs out of answers: the caller stops
    /// waiting instead of leaving the demo parked on an act no scripted
    /// client will answer.
    pub fn stop_when_exhausted(mut self, stop: Arc<tokio::sync::Notify>) -> Self {
        self.exhausted = Some(stop);
        self
    }

    /// The script had no line left for an act: the rest is left to the engine
    /// and the caller is told it may stop waiting.
    fn script_ran_out(&self) {
        if let Some(stop) = &self.exhausted {
            stop.notify_one();
        }
    }

    /// Bind the demo's human actors to the running engine: every directory
    /// user plus the supervisor `boss`, each an authenticated principal the
    /// menu actions run as.
    pub fn bind(&mut self, engine: &Engine, users: &[String]) {
        let mut ids: Vec<String> = users.to_vec();
        ids.push("boss".to_string());
        self.users = ids.clone();
        for id in ids {
            let principal = Principal::from_policy(&UserPolicy {
                name: id.clone(),
                enabled: true,
                allow: ACTOR_ACTIONS.iter().map(|a| a.to_string()).collect(),
                ..Default::default()
            })
            .expect("the demo actor policies are valid");
            self.actors
                .lock()
                .unwrap()
                .insert(id, (*engine.executor(&principal)).clone());
        }
    }

    /// The authenticated executor of one demo user.
    fn actor(&self, id: &str) -> Executor {
        self.actors
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| panic!("no demo actor '{id}' — the bind step builds them"))
    }

    /// The directory entries a user-picking prompt offers: the demo's users
    /// plus `boss`, least `except` (the act's own decider cannot urge it).
    fn user_items(&self, except: Option<&str>) -> Vec<String> {
        self.users
            .iter()
            .filter(|user| Some(user.as_str()) != except)
            .cloned()
            .collect()
    }

    /// The expense amount the run judges — the demo's one start input, the
    /// first line a script must answer. An empty answer keeps the model's own
    /// default (800). An interactive session asks again on a non-number; a
    /// scripted one takes the default instead (a script cannot answer again,
    /// and eating the choices that follow would drift the whole run).
    pub async fn ask_amount(&self) -> i64 {
        loop {
            let line = self.prompter.input("amount:").await;
            let line = line.trim().to_string();
            if line.is_empty() {
                return 800;
            }
            match line.parse::<i64>() {
                Ok(amount) => return amount,
                Err(_) => {
                    println!("   '{line}' is not an amount — enter a plain integer");
                    if matches!(self.prompter, Prompter::Script { .. }) {
                        println!(
                            "   the script's first line is the amount; keeping the default 800"
                        );
                        return 800;
                    }
                }
            }
        }
    }

    /// The executor whose decision the act currently waits on: the owner at
    /// creation, or whoever the latest assignment notice points at (a
    /// transfer's new owner, a delegation's delegatee). An act with no
    /// recorded decider keeps the legacy authority — any authenticated user
    /// decides it.
    fn decider(&self, tid: &str, system: &Executor) -> Executor {
        self.deciders
            .lock()
            .unwrap()
            .get(tid)
            .map(|id| self.actor(id))
            .unwrap_or_else(|| system.clone())
    }

    /// The run's graph and task rows: the facts every task menu reads.
    async fn view(
        &self,
        system: &Executor,
        pid: &str,
    ) -> std::result::Result<(acts::ProcessGraph, Vec<acts::TaskInfo>), String> {
        let graph = system
            .proc()
            .graph(pid)
            .await
            .map_err(|error| error.to_string())?;
        let info = system
            .proc()
            .get(pid)
            .await
            .map_err(|error| error.to_string())?;
        Ok((graph, info.tasks))
    }

    /// One engine message: the clerk's inbox. Assignment notifications and
    /// notices (a remind, an `acts.core.msg` delivery) are *projections*, not
    /// answers — they print the moment they land, on a detached task. A human
    /// act's menu runs detached too: the engine's per-run event worker is
    /// free while a menu is open, so a remind/transfer/delegate chosen on one
    /// act shows its notification immediately instead of queueing behind the
    /// menu. The choices themselves stay serialized on the clerk's gate, one
    /// action at a time.
    pub async fn process(self: &Arc<Self>, executor: &Executor, message: &Message) -> Result<()> {
        // The act's terminal state is recorded before anything else: a queued
        // menu keeps waiting, and the withdrawal that settles its act must
        // still reach the set while the menu is open.
        if message.is_type("step") {
            let state = message.state();
            if matches!(
                state,
                MessageState::Completed
                    | MessageState::Skipped
                    | MessageState::Cancelled
                    | MessageState::Aborted
                    | MessageState::Error
                    | MessageState::Removed
                    | MessageState::Backed
            ) {
                self.settled
                    .lock()
                    .unwrap()
                    .insert(message.tid.clone(), state.to_string());
            }
        }
        if message.r#type == "assignment" || message.r#type == "notice" {
            let client = self.clone();
            let message = message.clone();
            tokio::spawn(async move {
                if message.r#type == "assignment" {
                    println!(
                        "assignment: nid={} owner={:?} assignee={:?} delegator={:?} rev={}",
                        message.nid,
                        message.owner,
                        message.assignee,
                        message.delegator,
                        message.rev
                    );
                    if let Some(assignee) = &message.assignee {
                        client
                            .deciders
                            .lock()
                            .unwrap()
                            .insert(message.tid.clone(), assignee.clone());
                    }
                } else if message.is_msg() {
                    println!(
                        "notice: id={} inputs={} outputs={}",
                        message.tid, message.inputs, message.outputs
                    );
                } else {
                    let params = message.inputs.get::<Vars>("params").unwrap_or_default();
                    println!(
                        "remind: nid={} owner={:?} from={:?} message={:?} outputs={}",
                        message.nid,
                        message.owner,
                        params.get::<String>("from"),
                        params.get::<String>("message"),
                        message.outputs
                    );
                }
            });
            return Ok(());
        }

        if message.is_type("step") {
            if let Some(owner) = &message.owner {
                self.deciders
                    .lock()
                    .unwrap()
                    .insert(message.tid.clone(), owner.clone());
            }
            // A human act announces itself: put it to the terminal, detached
            // so the engine's event worker keeps draining the queue.
            if message.is_state(MessageState::Created) && message.is_irq() {
                let client = self.clone();
                let executor = executor.clone();
                let message = message.clone();
                tokio::spawn(async move {
                    client.interact(&executor, &message).await;
                });
            }
        }

        Ok(())
    }

    /// The act's terminal state when it already settled — withdrawn by a
    /// join, decided elsewhere, or rewound by a cancel: the menu then has
    /// nothing to offer.
    fn settled(&self, tid: &str) -> Option<String> {
        self.settled.lock().unwrap().get(tid).cloned()
    }

    /// One human act's decision menu: the client fills in `pid`, `tid` and
    /// the acting user (the act's current decider decides and signs), runs
    /// the choice, and returns once the act settled. A refused action prints
    /// and the menu asks again.
    async fn interact(&self, system: &Executor, message: &Message) {
        let pid = message.pid.clone();
        let tid = message.tid.clone();
        let nid = message.nid.clone();
        let key = message
            .inputs
            .get::<Vars>("params")
            .and_then(|params| params.get::<String>("key"))
            .unwrap_or_default();
        let owner = message.owner.clone();
        // One menu per instance: a re-announced `Created` never opens a
        // second one for the same act.
        if !self.menus.lock().unwrap().insert(tid.clone()) {
            return;
        }
        // Everything below returns through this guard: the instance's menu
        // is closed however the session ends.
        struct OpenMenu<'a>(&'a Mutex<HashSet<String>>, &'a str);
        impl Drop for OpenMenu<'_> {
            fn drop(&mut self) {
                self.0.lock().unwrap().remove(self.1);
            }
        }
        let _open = OpenMenu(&self.menus, tid.as_str());

        loop {
            // A queued act may have been withdrawn while this handler waited
            // — a `join: any` step passes on a sibling's vote and the join
            // skips the rest — or while the menu was open. A settled act
            // takes no further choice; the check runs every round.
            if let Some(state) = self.settled(&tid) {
                println!("== act {nid} already {state} — left to the engine");
                return;
            }
            // The decider is re-read every round: a transfer or delegation
            // moved it while the menu was open.
            let decider = self.decider(&tid, system);
            println!(
                "\n== act {nid} (key={key}) owner={} waits for a decision",
                owner.as_deref().unwrap_or("-")
            );
            // One arrow-driven list, rendered at the bottom of the
            // transcript; `complete` is the preselected default. Cancel is
            // always offered — it lists the current task's predecessors and
            // rewinds the one picked (the first act simply has none yet).
            let choices: Vec<(String, Menu)> = vec![
                ("complete".into(), Menu::Complete),
                ("add approver".into(), Menu::AddApprover),
                ("remove approver".into(), Menu::RemoveApprover),
                ("back".into(), Menu::Back),
                ("submit".into(), Menu::Submit),
                ("remind".into(), Menu::Remind),
                ("transfer".into(), Menu::Transfer),
                ("delegate".into(), Menu::Delegate),
                ("skip".into(), Menu::Skip),
                ("error".into(), Menu::Error),
                ("abort".into(), Menu::Abort),
                ("exit".into(), Menu::Leave),
                ("graph".into(), Menu::Graph),
                (
                    "cancel a predecessor task (undo the work ahead and re-run it)".into(),
                    Menu::Cancel,
                ),
            ];
            let items: Vec<&str> = choices.iter().map(|(label, _)| label.as_str()).collect();
            let Some(picked) = self.prompter.select("decide:", &items, 0).await else {
                println!("   the script ran out — the rest is left to the engine");
                self.script_ran_out();
                return;
            };
            // The choice's action runs on the clerk's gate: menus for
            // different acts may be open at once (the engine's event worker
            // stays free), but only one answer reaches the kernel at a time.
            let _clerk = self.gate.lock().await;
            match choices[picked].1 {
                Menu::Leave => return,
                Menu::Graph => match system.proc().graph(&pid).await {
                    Ok(graph) => println!("{}", graph.tree_output()),
                    Err(error) => println!("   refused: {error}"),
                },
                Menu::Cancel => match self.decide_cancel(system, &pid, &tid, &nid).await {
                    Ok(line) => {
                        println!("   {line}");
                        return;
                    }
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::Skip => {
                    let outcome = decider.act().skip(&pid, &tid, Vars::new()).await;
                    match outcome {
                        Ok(()) => {
                            println!("   skipped {nid}");
                            return;
                        }
                        Err(error) => println!("   refused: {error} — choose again"),
                    }
                }
                Menu::Error => match self.decide_error(&pid, &tid, &decider).await {
                    Ok(line) => {
                        println!("   {line}");
                        return;
                    }
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::Abort => {
                    let outcome = decider.act().abort(&pid, &tid, Vars::new()).await;
                    match outcome {
                        Ok(()) => {
                            println!("   aborted {nid}");
                            return;
                        }
                        Err(error) => println!("   refused: {error} — choose again"),
                    }
                }
                Menu::Complete => match self.decide_complete(&pid, &tid, &key, &decider).await {
                    Ok(line) => {
                        println!("   {line}");
                        return;
                    }
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::AddApprover => {
                    match self.decide_add_approver(&pid, &tid, system).await {
                        Ok((line, settled)) => {
                            println!("   {line}");
                            // A sign chain absorbs the target's own answer, so
                            // the menu ends; a plain push leaves the act's own
                            // decision due — the menu stays open for it.
                            if settled {
                                return;
                            }
                        }
                        Err(line) if line.contains("ActionNotAllowed") => {
                            println!("   the act already settled — left to the engine");
                            return;
                        }
                        Err(line) => println!("   {line} — choose again"),
                    }
                }
                Menu::RemoveApprover => {
                    match self.decide_remove_approver(&pid, &tid, system).await {
                        Ok(line) => println!("   {line}"),
                        Err(line) if line.contains("ActionNotAllowed") => {
                            println!("   the act already settled — left to the engine");
                            return;
                        }
                        Err(line) => println!("   {line} — choose again"),
                    }
                }
                Menu::Back => match self.decide_back(system, &pid, &tid, &nid, &decider).await {
                    Ok(line) => {
                        println!("   {line}");
                        return;
                    }
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::Submit => {
                    // A submit settles the act like a completion — the flow
                    // moves on, only the label differs — so the menu ends.
                    let outcome = decider.act().submit(&pid, &tid, Vars::new()).await;
                    if self.report(&outcome, &nid, "submitted") {
                        return;
                    }
                }
                Menu::Remind => match self.decide_remind(&pid, &tid).await {
                    Ok(line) => println!("   {line}"),
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::Transfer => match self.decide_transfer(&pid, &tid, &decider).await {
                    Ok(line) => println!("   {line}"),
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::Delegate => match self.decide_delegate(&pid, &tid, &decider).await {
                    Ok(line) => println!("   {line}"),
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
            }
        }
    }

    /// Print one action's outcome; `true` when the menu ends — the action
    /// decided the act, or the kernel refused it because the act settled
    /// elsewhere (a withdrawn vote, a rewound act) and the menu must stop
    /// instead of re-asking on a dead act.
    fn report(&self, outcome: &Result<()>, nid: &str, what: &str) -> bool {
        match outcome {
            Ok(()) => {
                println!("   {what} {nid}");
                true
            }
            Err(error) => refused_settled(error, nid),
        }
    }

    /// `complete`: the current decider decides; the outputs the flow reads
    /// are filled in (the board chair says who voted, the votes say who
    /// answered).
    async fn decide_complete(
        &self,
        pid: &str,
        tid: &str,
        key: &str,
        decider: &Executor,
    ) -> std::result::Result<String, String> {
        let mut vars = Vars::new();
        if key == "board_chair" {
            vars = vars.with("role", json!("chair"));
        }
        if let Some(decider_id) = self.deciders.lock().unwrap().get(tid) {
            vars = vars.with("uid", json!(decider_id));
        }
        decider
            .act()
            .complete(pid, tid, vars)
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!("completed {tid}"))
    }

    /// `add approver`: the user picks a task of the current node's subtree —
    /// the node's own task (a container's act) and every act the node
    /// admitted or had pushed under it — and the approver attaches there.
    /// The kernel creates the added approver differently by whether that task
    /// has subtasks: a task with children (a container, or an act already
    /// carrying pushed acts) takes the classic `push` (a new child act the
    /// join counts, owned by the approver), while a leaf with none takes the
    /// single-owner approver chain (`owners` + `mode`, before or after its
    /// decider). Returns the report line and whether the push settled *this*
    /// act: the chain absorbs its target's own answer, so the menu ends when
    /// the target is this act; a child push leaves the decision due.
    async fn decide_add_approver(
        &self,
        pid: &str,
        tid: &str,
        system: &Executor,
    ) -> std::result::Result<(String, bool), String> {
        let (graph, tasks) = self.view(system, pid).await?;
        let root = node_root(&graph, tid)
            .ok_or_else(|| format!("cannot locate the node of task {tid}"))?;
        let mut path = node_subtree(&graph, &tasks, &root);
        if path.is_empty() {
            return Err(format!("no task under {root}"));
        }
        // The act the menu is on is the natural target; keep it first and use
        // it as the default even when the node's own task leads the tree.
        path.sort_by_key(|entry| entry.tid != tid);
        let root_name = path
            .iter()
            .find(|entry| entry.tid == root)
            .map(|entry| entry.display_name().to_string())
            .unwrap_or_else(|| root.clone());
        print_entries(
            &format!(
                "add approver: the tasks under {root_name}; pick the one that gains the approver:"
            ),
            &path,
        );
        let labels: Vec<String> = path.iter().map(HistoryEntry::label).collect();
        let keys = history_keys(&path);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let Some(picked) = self
            .prompter
            .select_keyed("which task:", &label_refs, &key_refs, 0)
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let target = path[picked].clone();
        let here = format!("{} (task {})", target.display_name(), target.tid);
        // A task with subtasks is not a single owner: the added approver is a
        // child act under it. A task without them (an owned leaf) takes the
        // sign chain, which needs the owner to place the sign around.
        let has_subtasks = node_subtree(&graph, &tasks, &target.tid).len() > 1;
        let assigned = self.deciders.lock().unwrap().get(&target.tid).cloned();
        let users = self.user_items(None);
        let user_refs: Vec<&str> = users.iter().map(String::as_str).collect();
        let Some(approver) = self.prompter.select("approver:", &user_refs, 0).await else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let approver = users[approver].clone();
        if has_subtasks || assigned.is_none() {
            // The classic push: a child act under the selected task, owned by
            // the approver so her own menu asks for the decision.
            let vars = Vars::new()
                .with("name", "added approver")
                .with("uses", "acts.core.irq")
                .with("params", Vars::new().with("key", "added"))
                .with("assign", json!({ "owner": approver }));
            let executor = assigned
                .as_deref()
                .map(|id| self.actor(id))
                .unwrap_or_else(|| system.clone());
            executor
                .act()
                .push(pid, &target.tid, vars)
                .await
                .map_err(|error| error.to_string())?;
            let why = if has_subtasks {
                "as a child act (the task has subtasks)"
            } else {
                "as a child act (the task has no decider to sign relative to)"
            };
            return Ok((
                format!("added approver {approver} under {here} {why}"),
                false,
            ));
        }
        // The single-owner approver chain: before or after the decider.
        let Some(mode) = self
            .prompter
            .select(
                "the added approver signs:",
                &["before the decider", "after the decider (default)"],
                1,
            )
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let assigned = assigned.ok_or_else(|| "the target has no decider".to_string())?;
        let mut vars = Vars::new().with("owners", json!([approver]).to_string());
        if mode == 0 {
            vars = vars.with("mode", "before");
        }
        self.actor(&assigned)
            .act()
            .push(pid, &target.tid, vars)
            .await
            .map_err(|error| error.to_string())?;
        let position = if mode == 0 { "before" } else { "after" };
        Ok((
            format!("added approver {approver} {position} {here}"),
            target.tid == tid,
        ))
    }

    /// `remove approver`: pick an added approver under the current node — the
    /// chain leaves under a leaf and the child acts under a container alike —
    /// and drop it; the join recomputes and the node keeps waiting.
    async fn decide_remove_approver(
        &self,
        pid: &str,
        tid: &str,
        system: &Executor,
    ) -> std::result::Result<String, String> {
        let (graph, tasks) = self.view(system, pid).await?;
        let root = node_root(&graph, tid)
            .ok_or_else(|| format!("cannot locate the node of task {tid}"))?;
        let root_name = graph
            .nodes
            .iter()
            .find(|node| node.tid.as_deref() == Some(root.as_str()))
            .map(|node| node.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| root.clone());
        let leaves: Vec<HistoryEntry> = node_subtree(&graph, &tasks, &root)
            .into_iter()
            .filter(|entry| entry.nid.contains("#push"))
            .collect();
        if leaves.is_empty() {
            return Err(format!("no added approver under {root_name}"));
        }
        print_entries(
            &format!("remove approver: the added approvers under {root_name}:"),
            &leaves,
        );
        let labels: Vec<String> = leaves.iter().map(HistoryEntry::label).collect();
        let keys = history_keys(&leaves);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let Some(picked) = self
            .prompter
            .select_keyed("remove which:", &label_refs, &key_refs, 0)
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let leaf = leaves[picked].clone();
        // Remove answers to the nearest assigned execution *above* a pushed
        // act (the kernel's `approver_manager` starts at the parent), so the
        // manager is the first ancestor with a known decider, never the
        // pushed leaf's own owner.
        let by_tid = nodes_by_tid(&graph);
        let mut manager = None;
        let mut current = by_tid
            .get(leaf.tid.as_str())
            .and_then(|node| node.parent_tid.clone());
        while let Some(pid) = current {
            if let Some(id) = self.deciders.lock().unwrap().get(&pid).cloned() {
                manager = Some(id);
                break;
            }
            current = by_tid
                .get(pid.as_str())
                .and_then(|node| node.parent_tid.clone());
        }
        let remover = manager
            .map(|id| self.actor(&id))
            .unwrap_or_else(|| system.clone());
        remover
            .act()
            .remove(pid, &leaf.tid, Vars::new())
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!(
            "removed {} (task {}) from {root_name}",
            leaf.display_name(),
            leaf.tid
        ))
    }

    /// `remind`: a non-decider urges the act; the pick leaves the decider
    /// out (the kernel refuses their own urge anyway) and defaults to
    /// `boss`.
    async fn decide_remind(&self, pid: &str, tid: &str) -> std::result::Result<String, String> {
        let decider_id = self.deciders.lock().unwrap().get(tid).cloned();
        let candidates = self.user_items(decider_id.as_deref());
        if candidates.is_empty() {
            return Err("no other user can urge this act".to_string());
        }
        let default = candidates
            .iter()
            .position(|user| user == "boss")
            .unwrap_or(0);
        let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let Some(picked) = self
            .prompter
            .select("who urges:", &candidate_refs, default)
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let from = candidates[picked].clone();
        let note = self.prompter.input("note (optional):").await;
        let mut vars = Vars::new();
        if !note.is_empty() {
            vars = vars.with("message", note);
        }
        self.actor(&from)
            .act()
            .remind(pid, tid, vars)
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!("{from} urged {tid}"))
    }

    /// `transfer`: move the act's owner to another user; the act stays
    /// waiting and the new owner decides it.
    async fn decide_transfer(
        &self,
        pid: &str,
        tid: &str,
        decider: &Executor,
    ) -> std::result::Result<String, String> {
        let users = self.user_items(None);
        let user_refs: Vec<&str> = users.iter().map(String::as_str).collect();
        let Some(to) = self.prompter.select("transfer to:", &user_refs, 0).await else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let to = users[to].clone();
        decider
            .act()
            .transfer(
                pid,
                tid,
                Vars::new()
                    .with("to", to.clone())
                    .with("reason", "demo: transferred in the menu"),
            )
            .await
            .map_err(|error| error.to_string())?;
        // The detached assignment notice confirms the new owner and refreshes
        // the decider map on its own; record it here as well so the very next
        // choice already runs as the new owner, whatever order the tasks get.
        self.deciders
            .lock()
            .unwrap()
            .insert(tid.to_string(), to.clone());
        Ok(format!("transferred {tid} to {to}"))
    }

    /// `delegate`: let another user decide the act for its owner.
    async fn decide_delegate(
        &self,
        pid: &str,
        tid: &str,
        decider: &Executor,
    ) -> std::result::Result<String, String> {
        let users = self.user_items(None);
        let user_refs: Vec<&str> = users.iter().map(String::as_str).collect();
        let Some(delegatee) = self.prompter.select("delegate to:", &user_refs, 0).await else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let delegatee = users[delegatee].clone();
        decider
            .act()
            .delegate(
                pid,
                tid,
                Vars::new()
                    .with("delegatee", delegatee.clone())
                    .with("reason", "demo: delegated in the menu"),
            )
            .await
            .map_err(|error| error.to_string())?;
        // See `decide_transfer`: record the delegatee here so the next
        // choice already runs as them; the detached notice confirms it.
        self.deciders
            .lock()
            .unwrap()
            .insert(tid.to_string(), delegatee.clone());
        Ok(format!("delegated {tid} to {delegatee}"))
    }

    /// `back`: return the flow to a task the flow ran before this act's node
    /// — the predecessor list excludes this act's own node (the act and every
    /// execution enclosing it), its subtasks, the kernel groupings and the
    /// workflow record. The rewind closes this act's generation and re-enters
    /// the named node, which re-runs as a *new* instance: its new `Created`
    /// carries a different `tid`, and the menu asks again on it. The nearest
    /// predecessor is the default.
    async fn decide_back(
        &self,
        system: &Executor,
        pid: &str,
        tid: &str,
        nid: &str,
        decider: &Executor,
    ) -> std::result::Result<String, String> {
        let (graph, tasks) = self.view(system, pid).await?;
        // A pushed act is not a flow step: the kernel's rewind starts at the
        // node's own act, and a sign chain has already parked that act. The
        // added approver is completed or removed, never rewound.
        if !tasks.iter().any(|task| task.id == tid) {
            return Err(format!(
                "{nid} is an added approver — back applies to the flow task it hangs under; \
                 complete it, or remove it with remove approver"
            ));
        }
        let path = predecessor_tasks(&graph, &tasks, tid)?;
        print_entries(
            "back: pick the predecessor task the flow returns to (nearest first):",
            &path,
        );
        let labels: Vec<String> = path.iter().map(HistoryEntry::label).collect();
        let keys = history_keys(&path);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let Some(picked) = self
            .prompter
            .select_keyed("return to which task:", &label_refs, &key_refs, 0)
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let target = &path[picked];
        decider
            .act()
            .back(pid, tid, Vars::new().with("to", target.nid.clone()))
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!(
            "returned {nid} to {} (task {}) — it re-runs as a new instance (acts announced before the rewind may still ask: leave them to the engine)",
            target.display_name(),
            target.tid
        ))
    }

    /// `error`: fail the act with the operator's code and message — the
    /// flow's error walk takes over (no `catches` on this model, so the run
    /// errors out).
    async fn decide_error(
        &self,
        pid: &str,
        tid: &str,
        decider: &Executor,
    ) -> std::result::Result<String, String> {
        let code = self.prompter.input("error code (empty: rejected):").await;
        let code = if code.is_empty() {
            "rejected".to_string()
        } else {
            code
        };
        let message = self.prompter.input("error message (optional):").await;
        let mut vars = Vars::new().with("ecode", code.clone());
        if !message.is_empty() {
            vars = vars.with("message", message);
        }
        decider
            .act()
            .fail(pid, tid, vars)
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!("failed {tid} with {code}"))
    }

    /// `cancel`: cancel a completed predecessor — the kernel allows `Cancel`
    /// on a `Completed`/`Submitted` act, undoes the work ahead of it and
    /// re-enters that node as a *new* instance (a different `tid`); the menu
    /// asks again on the redo. The current act's node and its subtasks are
    /// excluded, and only a task the kernel can rewind is offered; the nearest
    /// one is the default.
    async fn decide_cancel(
        &self,
        system: &Executor,
        pid: &str,
        tid: &str,
        nid: &str,
    ) -> std::result::Result<String, String> {
        let (graph, tasks) = self.view(system, pid).await?;
        let path: Vec<HistoryEntry> = predecessor_tasks(&graph, &tasks, tid)?
            .into_iter()
            .filter(|entry| matches!(entry.state.as_str(), "completed" | "submitted"))
            .collect();
        if path.is_empty() {
            return Err(format!(
                "no completed predecessor task before {nid} to cancel"
            ));
        }
        print_entries(
            "cancel: pick the completed predecessor to undo (the work ahead re-runs):",
            &path,
        );
        let labels: Vec<String> = path.iter().map(HistoryEntry::label).collect();
        let keys = history_keys(&path);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let Some(picked) = self
            .prompter
            .select_keyed("cancel which task:", &label_refs, &key_refs, 0)
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let target = &path[picked];
        let executor = self
            .deciders
            .lock()
            .unwrap()
            .get(&target.tid)
            .map(|id| self.actor(id))
            .unwrap_or_else(|| system.clone());
        executor
            .act()
            .cancel(pid, &target.tid, Vars::new())
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!(
            "cancelled {} (task {}) from {nid} — it re-runs as a new instance (acts announced before the rewind may still ask: leave them to the engine)",
            target.display_name(),
            target.tid
        ))
    }
}
