//! The interactive demo client: every human act the engine announces is put
//! to the terminal as an arrow-key menu (complete / add approver / remove
//! approver / back / submit / remind / transfer / delegate / cancel /
//! graph); the client fills in `pid`, `tid` and the acting user, runs the
//! choice and waits for the next act, until the workflow completes. The
//! answers that need no user live in `sys.rs`. A `--script` replays one
//! recorded choice per prompt; a `--record` session writes exactly that
//! file, so a misbehaving run hands over its own repro.

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

/// One execution on a run's history path: the task instance (`tid`) and the
/// node it runs (`nid`, `name`).
#[derive(Clone, Debug)]
struct HistoryEntry {
    nid: String,
    tid: String,
    name: String,
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
}

/// The history path from one execution back to the run's root: the task
/// predecessor chain when the row names one (the flow order), and the
/// execution's parent when it does not (a pushed act). Virtual `block`
/// containers are walked through but not listed — they are the kernel's own
/// grouping, not a node a user acts on.
fn history_path(
    graph: &acts::ProcessGraph,
    tasks: &[acts::TaskInfo],
    start: &str,
) -> Vec<HistoryEntry> {
    let mut by_tid: HashMap<&str, &acts::GraphNode> = HashMap::new();
    for node in &graph.nodes {
        if let Some(tid) = node.tid.as_deref() {
            by_tid.insert(tid, node);
        }
    }
    let mut path = Vec::new();
    let mut current = start.to_string();
    let mut seen = HashSet::new();
    while seen.insert(current.clone()) {
        let Some(task) = tasks.iter().find(|task| task.id == current) else {
            break;
        };
        let node = by_tid.get(current.as_str()).copied();
        if node.is_none_or(|node| node.kind != "block") {
            // An unnamed act (a pushed vote, a fan-out leaf) carries the
            // action key it runs in its params; that key is the name a user
            // knows it by, and a stable script key across runs.
            let name = if task.name.is_empty() {
                node.and_then(|node| node.params.get("key"))
                    .and_then(|key| key.as_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                task.name.clone()
            };
            path.push(HistoryEntry {
                nid: task.nid.clone(),
                tid: task.id.clone(),
                name,
            });
        }
        // The row's `prev` is the node the flow came from; with several
        // generations of it, the latest one that started before this
        // execution is the one it inherited. A pushed act has no predecessor
        // row — its graph parent is the act it hangs under.
        let previous = task.prev.as_ref().and_then(|prev| {
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

/// The stable script keys for a history menu: the node name while it is
/// unique on the path, and `name#<index>` for a name that appears several
/// times (a fan-out's repeated act), so `--record` never writes a tid that
/// the next run cannot match.
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

/// Print one history path, the first entry marked with `first` (the act the
/// menu is about); auxiliary evidence for rewinds and sign-chain actions.
fn print_history(path: &[HistoryEntry], note: &str, first: &str) {
    println!("   {note}");
    for (index, entry) in path.iter().enumerate() {
        let here = if index == 0 {
            format!("  <- {first}")
        } else {
            String::new()
        };
        println!("     {}  task={}{here}", entry.display_name(), entry.tid);
    }
}

#[derive(Default)]
pub struct Client {
    /// node id → `(pid, tid)` of every step instance the demo has seen, so
    /// a menu action can address a pushed act by the node id it printed.
    seen: Mutex<HashMap<String, (String, String)>>,
    /// tid → the user whose decision the act currently waits on: the owner
    /// at creation, refreshed by every assignment notice (a transfer moves
    /// it, a delegation points it at the delegatee).
    deciders: Mutex<HashMap<String, String>>,
    /// The last act that completed: `(nid, pid, tid)` — `cancel`'s default
    /// target, the node the flow just moved past.
    last_completed: Mutex<Option<(String, String, String)>>,
    /// authenticated executors per user id, built once at `bind` — the
    /// kernel attributes and authorizes every menu action by the subject.
    actors: Mutex<HashMap<String, Executor>>,
    /// One clerk processes the inbox: the demo answers one message at a
    /// time, so two answers never race in the same act's commit window.
    gate: tokio::sync::Mutex<()>,
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

    /// The action just decided this act: record it as the cancel target now,
    /// not only when its `Completed` message reaches the clerk — the next
    /// act's menu can open before that message, and `cancel` must point at the
    /// work just done.
    fn note_completed(&self, nid: &str, pid: &str, tid: &str) {
        *self.last_completed.lock().unwrap() =
            Some((nid.to_string(), pid.to_string(), tid.to_string()));
    }

    /// The history path from one act back to the run's root, as the task rows
    /// and the graph agree on it: each entry carries the task id and the node
    /// name the menus print.
    async fn history(
        &self,
        system: &Executor,
        pid: &str,
        tid: &str,
    ) -> std::result::Result<Vec<HistoryEntry>, String> {
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
        let path = history_path(&graph, &info.tasks, tid);
        if path.is_empty() {
            return Err(format!("no task history for {tid}"));
        }
        Ok(path)
    }

    pub async fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        // The act's terminal state is recorded before the clerk's gate: a
        // queued menu keeps the gate, and the withdrawal that settles its
        // act must still reach the set while the menu is open.
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
        // The clerk takes the gate next: every action below runs one at a
        // time.
        let _clerk = self.gate.lock().await;

        // A transfer/delegation notice is its own event: print the assignment
        // it projects, refresh the act's current decider, and never answer it
        // like the act's Created message.
        if message.r#type == "assignment" {
            println!(
                "assignment: nid={} owner={:?} assignee={:?} delegator={:?} delegatee={:?} rev={}",
                message.nid,
                message.owner,
                message.assignee,
                message.delegator,
                message.delegatee,
                message.rev
            );
            if let Some(assignee) = &message.assignee {
                self.deciders
                    .lock()
                    .unwrap()
                    .insert(message.tid.clone(), assignee.clone());
            }
            return Ok(());
        }

        // A remind record is the urge a user wrote on a pending act: it
        // addresses the act's owner and decides nothing. The caller and the
        // note ride `inputs.params`.
        if message.r#type == "remind" {
            let params = message.inputs.get::<Vars>("params").unwrap_or_default();
            println!(
                "remind: nid={} owner={:?} from={:?} message={:?}",
                message.nid,
                message.owner,
                params.get::<String>("from"),
                params.get::<String>("message")
            );
            return Ok(());
        }

        if message.is_type("step") {
            // Track the instance so menu actions can name a pushed act by
            // the node id it printed, and know whose decision it waits on.
            self.seen.lock().unwrap().insert(
                message.nid.clone(),
                (message.pid.clone(), message.tid.clone()),
            );
            if let Some(owner) = &message.owner {
                self.deciders
                    .lock()
                    .unwrap()
                    .insert(message.tid.clone(), owner.clone());
            }
            // The last act that completed — `cancel`'s default target —
            // skips the system-answered requests and the virtual containers:
            // only a human act (irq) is a node a user would re-run.
            let sys_answered = matches!(
                message.uses.as_deref(),
                Some("acts.core.users") | Some("acts.core.decide") | Some("acts.core.block")
            );
            if message.is_state(MessageState::Completed) && !sys_answered && message.is_irq() {
                *self.last_completed.lock().unwrap() = Some((
                    message.nid.clone(),
                    message.pid.clone(),
                    message.tid.clone(),
                ));
            }
            // A human act announces itself: put it to the terminal.
            if message.is_state(MessageState::Created) && message.is_irq() {
                self.interact(executor, message).await;
            }
        }

        if message.is_type("msg") {
            println!("msg: id={} inputs={}", message.tid, message.inputs);
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
            // transcript; `complete` is the preselected default. Cancel
            // rides last: it rewinds the step the flow just moved past
            // (after the first step there is always one; at the very first
            // act there is nothing to cancel yet).
            let mut choices: Vec<(String, Menu)> = vec![
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
                ("leave it to the engine".into(), Menu::Leave),
                ("show the run's graph".into(), Menu::Graph),
            ];
            if let Some(previous) = self
                .last_completed
                .lock()
                .unwrap()
                .as_ref()
                .map(|(nid, _, _)| nid.clone())
            {
                choices.push((
                    format!("cancel {previous} (undo the work ahead and re-run it)"),
                    Menu::Cancel,
                ));
            }
            let items: Vec<&str> = choices.iter().map(|(label, _)| label.as_str()).collect();
            let Some(picked) = self.prompter.select("decide:", &items, 0).await else {
                println!("   the script ran out — the rest is left to the engine");
                self.script_ran_out();
                return;
            };
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
                        self.note_completed(&nid, &pid, &tid);
                        return;
                    }
                    Err(line) if line.contains("ActionNotAllowed") => {
                        println!("   the act already settled — left to the engine");
                        return;
                    }
                    Err(line) => println!("   {line} — choose again"),
                },
                Menu::AddApprover => {
                    match self.decide_add_approver(&pid, &tid, &decider, system).await {
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
                    match self.decide_remove_approver(&nid, &pid, &tid, system).await {
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
                    if outcome.is_ok() {
                        self.note_completed(&nid, &pid, &tid);
                    }
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

    /// `add approver`: an owned act gains a sign chain the user places before
    /// or after its current decider (`acts.core.push` modes `before`/`after`);
    /// an unassigned act gains a plain member act the join counts. The sign
    /// position is asked first, on every act: `1) before  2) after` — on an
    /// act with no decider there is no one to sign relative to, so the choice
    /// is reported as inert and the push hangs a plain co-signer instead.
    /// Returns the report line and whether the push settled the act: a sign
    /// chain absorbs the target's own answer (the menu ends), a plain push
    /// leaves it due (the menu stays open).
    async fn decide_add_approver(
        &self,
        pid: &str,
        tid: &str,
        decider: &Executor,
        system: &Executor,
    ) -> std::result::Result<(String, bool), String> {
        // The sign chain hangs on this act; print where it sits on the path
        // back to the root, task id and node name on every entry.
        let history = self.history(system, pid, tid).await;
        match &history {
            Ok(path) => print_history(
                path,
                "add approver: the sign chain hangs on this act; history to the root:",
                "this act",
            ),
            Err(error) => println!("   history unavailable: {error}"),
        }
        let here = history
            .ok()
            .and_then(|path| path.into_iter().next())
            .map(|entry| format!("{} (task {})", entry.display_name(), entry.tid))
            .unwrap_or_else(|| format!("task {tid}"));
        // The two sign positions, spelled out first: the added approver signs
        // before the current decider, or after her (the default).
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
        let users = self.user_items(None);
        let user_refs: Vec<&str> = users.iter().map(String::as_str).collect();
        let Some(approver) = self.prompter.select("approver:", &user_refs, 0).await else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let approver = users[approver].clone();
        let assigned = self.deciders.lock().unwrap().contains_key(tid);
        let mut vars = Vars::new();
        let executor = if assigned {
            vars = vars.with("owners", json!([approver]).to_string());
            if mode == 0 {
                vars = vars.with("mode", "before");
            }
            decider.clone()
        } else {
            vars = vars
                .with("name", "added approver")
                .with("uses", "acts.core.irq")
                .with("params", Vars::new().with("key", "added"));
            system.clone()
        };
        executor
            .act()
            .push(pid, tid, vars)
            .await
            .map_err(|error| error.to_string())?;
        if assigned {
            Ok((format!("added approver {approver} under {here}"), true))
        } else {
            Ok((
                format!(
                    "added approver {approver} under {here} as a plain co-signer \
                     (the act has no decider: before/after needs one, and the \
                     act's own decision is still due)"
                ),
                false,
            ))
        }
    }

    /// `remove approver`: pick one of the pushed acts under this act and
    /// drop it; the join recomputes and the act keeps waiting.
    async fn decide_remove_approver(
        &self,
        nid: &str,
        pid: &str,
        tid: &str,
        system: &Executor,
    ) -> std::result::Result<String, String> {
        // The removed act hangs under this one; print the path back to the
        // root so the report names the node the sign chain was added to and
        // the task id it is being removed from.
        let history = self.history(system, pid, tid).await;
        match &history {
            Ok(path) => print_history(
                path,
                "remove approver: the pushed acts hang on this act; history to the root:",
                "this act",
            ),
            Err(error) => println!("   history unavailable: {error}"),
        }
        let here = history
            .ok()
            .and_then(|path| path.into_iter().next())
            .map(|entry| format!("{} (task {})", entry.display_name(), entry.tid))
            .unwrap_or_else(|| format!("task {tid}"));
        let prefix = format!("{nid}#push");
        let mut leaves: Vec<(String, String, String)> = self
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, _)| seen.starts_with(&prefix))
            .map(|(seen, (pid, tid))| (seen.clone(), pid.clone(), tid.clone()))
            .collect();
        leaves.sort();
        if leaves.is_empty() {
            return Err(format!("no pushed act under {nid}"));
        }
        let items: Vec<String> = leaves
            .iter()
            .map(|(seen, _, leaf_tid)| format!("{seen}  task={leaf_tid}"))
            .collect();
        let item_refs: Vec<&str> = items.iter().map(String::as_str).collect();
        let keys: Vec<&str> = leaves.iter().map(|(seen, _, _)| seen.as_str()).collect();
        let Some(picked) = self
            .prompter
            .select_keyed("remove which:", &item_refs, &keys, 0)
            .await
        else {
            return Err("the script ran out — the rest is left to the engine".to_string());
        };
        let Some((seen, leaf_pid, leaf_tid)) = leaves
            .get(picked)
            .map(|(seen, pid, tid)| (seen.clone(), pid.clone(), tid.clone()))
        else {
            return Err("pick a listed act".to_string());
        };
        let remover = self
            .deciders
            .lock()
            .unwrap()
            .get(tid)
            .map(|id| self.actor(id))
            .unwrap_or_else(|| system.clone());
        remover
            .act()
            .remove(&leaf_pid, &leaf_tid, Vars::new())
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!("removed {seen} (task {leaf_tid}) from {here}"))
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
        // The assignment notice that confirms the new owner is queued behind
        // this very menu (the clerk's gate); record the decider now so the
        // next choice runs as the new owner.
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
        // See `decide_transfer`: the notice is queued behind this menu, so
        // the delegatee is recorded as the act's decider right away.
        self.deciders
            .lock()
            .unwrap()
            .insert(tid.to_string(), delegatee.clone());
        Ok(format!("delegated {tid} to {delegatee}"))
    }

    /// `back`: return the flow to an earlier task on this act's history path
    /// to the root. The rewind closes this act's generation and re-enters the
    /// named node, which re-runs as a *new* instance: its new `Created`
    /// carries a different `tid`, and the menu asks again on it. The menu
    /// names every entry by its task id and node name, so one instance among
    /// several generations can be picked; the nearest predecessor is the
    /// default.
    async fn decide_back(
        &self,
        system: &Executor,
        pid: &str,
        tid: &str,
        nid: &str,
        decider: &Executor,
    ) -> std::result::Result<String, String> {
        let path = self.history(system, pid, tid).await?;
        print_history(
            &path,
            "back: this act's history to the root (nearest predecessor first):",
            "this act",
        );
        let labels: Vec<String> = path.iter().map(HistoryEntry::label).collect();
        let keys = history_keys(&path);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        // The act itself is entry 0; its nearest predecessor is the default.
        let default = usize::from(path.len() > 1);
        let Some(picked) = self
            .prompter
            .select_keyed(
                "return to which task (back to the root):",
                &label_refs,
                &key_refs,
                default,
            )
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

    /// `cancel`: issued on the previous node that has already completed —
    /// the kernel allows `Cancel` on a `Completed`/`Submitted` act, undoes
    /// the work ahead of it and re-enters that node as a *new* instance (a
    /// different `tid`); the menu asks again on the redo. The default target
    /// is the act the flow just moved past; the kernel requires live forward
    /// work to rewind and refuses a settled chain.
    async fn decide_cancel(
        &self,
        system: &Executor,
        pid: &str,
        tid: &str,
        nid: &str,
    ) -> std::result::Result<String, String> {
        // The last act the flow moved past is the default target and the
        // start of the history path; at the very first act there is none, so
        // the current act's own path is the best available listing.
        let last = self.last_completed.lock().unwrap().clone();
        let start = last
            .as_ref()
            .filter(|(_, target_pid, target_tid)| target_pid == pid && target_tid != tid)
            .map(|(_, _, target_tid)| target_tid.clone())
            .unwrap_or_else(|| tid.to_string());
        let path = self.history(system, pid, &start).await?;
        let marker = if start == tid {
            "this act"
        } else {
            "last completed"
        };
        print_history(
            &path,
            "cancel: the history to the root (the default target first):",
            marker,
        );
        let labels: Vec<String> = path.iter().map(HistoryEntry::label).collect();
        let keys = history_keys(&path);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let Some(picked) = self
            .prompter
            .select_keyed(
                "cancel which task (back to the root):",
                &label_refs,
                &key_refs,
                0,
            )
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
