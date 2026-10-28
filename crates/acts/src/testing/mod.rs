//! The functional-test harness, exported for downstream crates' tests.
//!
//! [`FlowFixture`] deploys a workflow on an engine, starts it, waits on the
//! messages the run announces, serves the client's actions and reads the run's
//! outcomes. It is the harness the kernel's own cells use; a package or plugin
//! crate can depend on `acts` with the `test-utils` feature and drive its
//! integration tests through the same public engine API instead of rebuilding
//! the plumbing.
//!
//! The module is gated: it is compiled for the crate's own tests and for a
//! dependent that enables `test-utils`, and never in a normal build. A package
//! or plugin crate enables it in its dev-dependencies:
//!
//! ```toml
//! [dev-dependencies]
//! acts = { workspace = true, features = ["test-utils"] }
//! ```

use parking_lot::Mutex;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::Instant;

#[cfg(test)]
use crate::scheduler::{ProcRead, ProcessSnapshot, TaskRow, TaskState};
use crate::{
    Action, Config, Engine, Message, MessageState, Principal, Vars, Workflow, event::EventAction,
    utils::consts,
};

/// The message a client addresses: `tid` is the execution instance, `nid` the
/// model node.
pub type Created = Message;

/// How long a wait polls before it fails the test.
const WAIT: Duration = Duration::from_secs(90);
/// How long a wait sleeps between polls.
const TICK: Duration = Duration::from_millis(20);

/// How a run ended, from its lifecycle channel.
#[cfg(test)]
#[derive(Debug)]
pub(crate) enum Terminal {
    Completed(Message),
    Errored(Message),
}

/// One deployed model and one run of it, with its emitted messages.
pub struct FlowFixture {
    engine: Engine,
    workflow: Workflow,
    pid: String,
    messages: Arc<Mutex<Vec<Message>>>,
    completions: Arc<Mutex<Vec<Message>>>,
    errors: Arc<Mutex<Vec<Message>>>,
}

impl FlowFixture {
    /// Deploy `workflow` on a fresh engine with the default (test) config.
    pub async fn deploy(workflow: &Workflow) -> Self {
        Self::deploy_with(workflow, &Config::default()).await
    }

    /// Deploy `workflow` with the cell's own config (a run bound, a tick).
    pub async fn deploy_with(workflow: &Workflow, config: &Config) -> Self {
        let engine = Engine::builder().set_config(config).start().await.unwrap();
        Self::deploy_on(engine, workflow).await
    }

    /// Deploy `workflow` on an engine the caller built — the shape a package or
    /// plugin crate tests with its own packages/plugins installed.
    pub async fn deploy_on(engine: Engine, workflow: &Workflow) -> Self {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let completions = Arc::new(Mutex::new(Vec::new()));
        let errors = Arc::new(Mutex::new(Vec::new()));
        {
            let sink = messages.clone();
            engine.channel().on_message(move |e| {
                let sink = sink.clone();
                async move {
                    sink.lock().push(e.inner().clone());
                }
            });
        }
        {
            let sink = completions.clone();
            engine.channel().on_complete(move |e| {
                let sink = sink.clone();
                async move {
                    sink.lock().push(e.inner().clone());
                }
            });
        }
        {
            let sink = errors.clone();
            engine.channel().on_error(move |e| {
                let sink = sink.clone();
                async move {
                    sink.lock().push(e.inner().clone());
                }
            });
        }
        engine
            .executor(&Principal::unrestricted())
            .model()
            .deploy(workflow, None)
            .await
            .unwrap();
        Self {
            engine,
            workflow: workflow.clone(),
            pid: String::new(),
            messages,
            completions,
            errors,
        }
    }

    /// Start the model and answer its pid.
    pub async fn start(&mut self, vars: Vars) -> String {
        self.try_start(vars).await.unwrap()
    }

    /// Start the model, preserving a refusal (an unsupported model declaration,
    /// a duplicate pid): the cell that pins the refusal does not need an engine
    /// of its own.
    pub async fn try_start(&mut self, vars: Vars) -> crate::Result<String> {
        let pid = self.engine.runtime().start(&self.workflow, vars).await?;
        self.pid = pid.clone();
        Ok(pid)
    }

    /// Start with the cell's own pid (the `pid` option), answering the pid.
    pub async fn start_with_pid(&mut self, pid: &str, vars: Vars) -> crate::Result<String> {
        let mut vars = vars;
        vars.set(consts::PROCESS_ID, pid);
        let started = self.engine.runtime().start(&self.workflow, vars).await?;
        self.pid = started.clone();
        Ok(started)
    }

    /// Start the model through a caller's principal — the path `proc:start`
    /// takes, so the run carries that caller's authority and `initiator`.
    pub async fn start_as(&mut self, principal: &Principal, vars: Vars) -> crate::Result<String> {
        let handle = self
            .engine
            .executor(principal)
            .proc()
            .start(&self.workflow.id, vars)
            .await?;
        self.pid = handle.pid().to_string();
        Ok(self.pid.clone())
    }

    /// Start and wait for the run's terminal lifecycle event.
    #[cfg(test)]
    pub(crate) async fn run(&mut self, vars: Vars) -> Terminal {
        self.start(vars).await;
        self.wait_terminal().await
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn pid(&self) -> &str {
        &self.pid
    }

    /// Every task/act message the engine announced, in delivery order.
    pub fn messages(&self) -> Vec<Message> {
        self.messages.lock().clone()
    }

    /// The messages of the model node `nid`.
    pub fn messages_of(&self, nid: &str) -> Vec<Message> {
        self.messages
            .lock()
            .iter()
            .filter(|message| message.nid == nid)
            .cloned()
            .collect()
    }

    /// The latest message of `nid` in `state`, if it was announced.
    pub fn message(&self, nid: &str, state: MessageState) -> Option<Message> {
        self.messages
            .lock()
            .iter()
            .rev()
            .find(|message| message.nid == nid && message.state == state)
            .cloned()
    }

    /// Wait until `nid` announces `state` and answer that message.
    pub async fn wait_message(&self, nid: &str, state: MessageState) -> Message {
        let what = format!("{nid} to announce {state:?}");
        poll_until(&what, || async { self.message(nid, state).is_some() }).await;
        self.message(nid, state)
            .unwrap_or_else(|| panic!("{what}: message disappeared"))
    }

    /// Wait for the `created` message that names the act to address.
    pub async fn wait_created(&self, nid: &str) -> Created {
        self.wait_message(nid, MessageState::Created).await
    }

    /// Wait for another `created` instance of `nid` (a fan-out creates one per
    /// item under the same model node): `except` is the instance already seen.
    pub async fn wait_created_except(&self, nid: &str, except: &str) -> Created {
        let what = format!("{nid} to announce another created instance");
        poll_until(&what, || async {
            self.messages_of(nid)
                .iter()
                .any(|message| message.state == MessageState::Created && message.tid != except)
        })
        .await;
        self.messages_of(nid)
            .into_iter()
            .find(|message| message.state == MessageState::Created && message.tid != except)
            .unwrap_or_else(|| panic!("{what}: instance disappeared"))
    }

    /// Wait until `nid` announced at least `count` messages and answer them in
    /// delivery order (fan-out items share a node id).
    pub async fn wait_messages(&self, nid: &str, count: usize) -> Vec<Message> {
        let what = format!("{count} messages of {nid}");
        poll_until(&what, || async { self.messages_of(nid).len() >= count }).await;
        self.messages_of(nid)
    }

    /// Wait until `uses` announced at least `count` created acts, and answer
    /// them in delivery order (a fan-out gives one act per item).
    pub async fn wait_created_uses(&self, uses: &str, count: usize) -> Vec<Message> {
        let what = format!("{count} created {uses} acts");
        poll_until(&what, || async { self.created_uses(uses).len() >= count }).await;
        self.created_uses(uses)
    }

    fn created_uses(&self, uses: &str) -> Vec<Message> {
        self.messages
            .lock()
            .iter()
            .filter(|message| {
                message.state == MessageState::Created && message.uses.as_deref() == Some(uses)
            })
            .cloned()
            .collect()
    }

    /// Every state `nid` announced, in order (the legacy per-act state trail).
    pub fn announced(&self, nid: &str) -> Vec<MessageState> {
        self.messages_of(nid)
            .into_iter()
            .map(|message| message.state)
            .collect()
    }

    /// Serve one client action on the instance addressed by `pid` + `tid` —
    /// the wire every client sends. The node id is not an address: an action
    /// can only name one execution instance.
    pub(crate) async fn decide(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: Vars,
    ) -> crate::Result<()> {
        self.engine
            .runtime()
            .do_action(&Action::new(pid, tid, action, options))
            .await
    }

    /// Serve `action` on one execution instance (`pid` + `tid`).
    pub(crate) async fn act(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: Vars,
    ) -> crate::Result<()> {
        self.decide(pid, tid, action, options).await
    }

    pub async fn next(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Next, Vars::new()).await
    }

    pub async fn submit(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Submit, Vars::new()).await
    }

    pub async fn skip(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Skip, Vars::new()).await
    }

    pub async fn abort(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Abort, Vars::new()).await
    }

    pub async fn fail(&self, pid: &str, tid: &str, code: &str, message: &str) -> crate::Result<()> {
        self.act(
            pid,
            tid,
            EventAction::Error,
            Vars::new()
                .with(consts::ACT_ERR_CODE, code)
                .with(consts::ACT_ERR_MESSAGE, message),
        )
        .await
    }

    pub async fn back(&self, pid: &str, tid: &str, to: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Back, Vars::new().with("to", to))
            .await
    }

    pub async fn cancel(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Cancel, Vars::new()).await
    }

    /// Complete an IRQ act with its outputs (the unrestricted system caller,
    /// the same path a test client uses).
    pub async fn complete(&self, pid: &str, tid: &str, outputs: Vars) {
        self.engine
            .executor(&Principal::unrestricted())
            .act()
            .complete(pid, tid, outputs)
            .await
            .unwrap();
    }

    /// Wait until the run reports a terminal lifecycle event.
    #[cfg(test)]
    pub(crate) async fn wait_terminal(&self) -> Terminal {
        let what = format!("run {} to report terminal", self.pid);
        poll_until(&what, || async {
            !self.errors.lock().is_empty() || !self.completions.lock().is_empty()
        })
        .await;
        if let Some(message) = self.completions.lock().last().cloned() {
            return Terminal::Completed(message);
        }
        Terminal::Errored(self.errors.lock().last().cloned().unwrap())
    }

    /// Wait for the run's completion and answer its lifecycle message.
    pub async fn wait_completed(&self) -> Message {
        let what = format!("run {} to complete", self.pid);
        poll_until(&what, || async { !self.completions.lock().is_empty() }).await;
        self.completions.lock().last().cloned().unwrap()
    }

    /// Wait for the run's error and answer its lifecycle message.
    pub async fn wait_error(&self) -> Message {
        let what = format!("run {} to error", self.pid);
        poll_until(&what, || async { !self.errors.lock().is_empty() }).await;
        self.errors.lock().last().cloned().unwrap()
    }

    /// The run's completion outputs (its model's `exposes`).
    pub fn outputs(&self) -> Vars {
        self.completions
            .lock()
            .last()
            .map(|message| message.outputs.clone())
            .unwrap_or_default()
    }

    /// The run's aggregate — the durable execution states.
    #[cfg(test)]
    pub(crate) async fn snapshot(&self) -> ProcessSnapshot {
        self.engine
            .runtime()
            .kernel()
            .snapshot(&self.pid)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("run {} has no aggregate", self.pid))
    }

    /// The newest execution state of the model node `nid`.
    #[cfg(test)]
    pub(crate) async fn state(&self, nid: &str) -> TaskState {
        let snapshot = self.snapshot().await;
        snapshot
            .tasks
            .iter()
            .filter(|(id, _)| id.node.as_str() == nid)
            .max_by_key(|(id, _)| id.generation)
            .map(|(_, task)| task.state)
            .unwrap_or_else(|| {
                panic!(
                    "no execution for node '{nid}': {:?}",
                    snapshot
                        .tasks
                        .keys()
                        .map(|id| id.node.as_str().to_owned())
                        .collect::<Vec<_>>()
                )
            })
    }

    /// Every node's newest execution state, sorted by node id.
    #[cfg(test)]
    pub(crate) async fn states(&self) -> Vec<(String, TaskState)> {
        let snapshot = self.snapshot().await;
        let mut states: std::collections::BTreeMap<String, (u64, TaskState)> =
            std::collections::BTreeMap::new();
        for (id, task) in &snapshot.tasks {
            let entry = states
                .entry(id.node.as_str().to_owned())
                .or_insert((id.generation, task.state));
            if id.generation >= entry.0 {
                *entry = (id.generation, task.state);
            }
        }
        states
            .into_iter()
            .map(|(node, (_, state))| (node, state))
            .collect()
    }

    /// Wait until `nid`'s newest execution is `state`.
    #[cfg(test)]
    pub(crate) async fn wait_state(&self, nid: &str, state: TaskState) {
        let what = format!("{nid} to reach {state:?}");
        poll_until(&what, || async { self.state(nid).await == state }).await;
    }

    /// Wait until every execution is terminal.
    #[cfg(test)]
    pub(crate) async fn wait_settled(&self) {
        let what = format!("run {} to settle", self.pid);
        poll_until(&what, || async {
            self.snapshot()
                .await
                .tasks
                .values()
                .all(|task| task.state.is_terminal())
        })
        .await;
    }

    /// The run's projected read model (`None` once reclaimed; never in the
    /// test build, where reclamation is explicit).
    #[cfg(test)]
    pub(crate) async fn rows(&self) -> Option<Arc<ProcRead>> {
        self.engine
            .runtime()
            .read_proc(&self.pid)
            .await
            .unwrap()
            .map(Arc::new)
    }

    /// The projected row of the model node `nid`.
    #[cfg(test)]
    pub(crate) async fn row(&self, nid: &str) -> TaskRow {
        let rows = self.rows().await.expect("the run has projected rows");
        rows.task_by_nid(nid)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no task row for node '{nid}'"))
    }

    pub async fn close(self) {
        self.engine.close().await;
    }
}

/// A model whose every step is package-less (it self-completes), so a run
/// reaches its terminal state without a client.
#[cfg(test)]
pub(crate) fn auto_workflow(id: &str, steps: &[&str]) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    for step in steps {
        let step = step.to_string();
        workflow.steps.push(crate::model::Step {
            id: step.clone(),
            name: step,
            ..Default::default()
        });
    }
    workflow
}

/// A one-step irq model keyed `key`: the client-actionable act.
#[cfg(test)]
pub(crate) fn irq_workflow(id: &str, step: &str, key: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(crate::model::Step {
        id: step.to_string(),
        name: step.to_string(),
        uses: Some("acts.core.irq".to_string()),
        params: Vars::new().with("key", key).to_value(),
        ..Default::default()
    });
    workflow
}

/// Poll `ready` until it holds or `WAIT` runs out.
pub(crate) async fn poll_until<F, Fut>(what: &str, mut ready: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + WAIT;
    loop {
        if ready().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: timed out after {WAIT:?}"
        );
        tokio::time::sleep(TICK).await;
    }
}
