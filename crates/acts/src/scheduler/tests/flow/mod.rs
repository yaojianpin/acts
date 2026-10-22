//! Functional scheduler cells: steps, branches, acts, vars and messages.
//!
//! These are the pre-kernel scheduler tests (`step/*`, `branch/*`, `act/*`,
//! `message`, `vars`, `proc`, `scher`), migrated to the kernel engine. Each
//! cell keeps the legacy goal but observes the kernel's own surfaces: the
//! aggregate snapshot for execution states, the engine's channel for the
//! messages a run announces, and the projected rows for the read model.
//!
//! [`FlowFixture`] is the shared harness. `deploy` prepares an engine and its
//! model, `start` runs it, awaits (`wait_created`, `wait_message`,
//! `wait_completed`, `wait_state`) do the synchronization, `decide`/`next`/
//! `submit`/`skip`/`abort`/`error`/`back`/`cancel` serve the client's action
//! on the message that named the act, and `state`/`outputs`/`messages_of` are
//! the assertions. A cell therefore reads as the behavior it pins instead of
//! as engine plumbing.
//!
//! The kernel never reclaims a run inside the test build (the terminal-cleanup
//! timer is disabled there), so a settled run's aggregate stays readable after
//! its completion event — the same durable evidence the legacy resident
//! process held in memory.

pub(crate) mod act;
pub(crate) mod branch;
pub(crate) mod catch;
pub(crate) mod message;
pub(crate) mod outputs;
pub(crate) mod runtime;
pub(crate) mod step;
pub(crate) mod subflow;
pub(crate) mod timeout;
pub(crate) mod vars;

use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    Action, Config, Engine, Message, MessageState, Principal, Vars, Workflow,
    event::EventAction,
    scheduler::{
        core::{commit::aggregate::ProcessSnapshot, model::task_graph::TaskState},
        shell::{ProcRead, TaskRow},
    },
    utils::consts,
};

use super::reliability::{poll_until, proc_row};

/// The message a client addresses: `tid` is the execution instance, `nid` the
/// model node. The fixture hands back the message the engine emitted, so an
/// action is always served on the exact instance that announced itself.
pub(crate) type Created = Message;

/// How a run ended, from its lifecycle channel.
#[derive(Clone, Debug)]
pub(crate) enum Terminal {
    Completed(Message),
    Errored(Message),
}

/// One deployed model and one run of it, with its emitted messages.
pub(crate) struct FlowFixture {
    engine: Engine,
    workflow: Workflow,
    pid: String,
    messages: Arc<Mutex<Vec<Message>>>,
    completions: Arc<Mutex<Vec<Message>>>,
    errors: Arc<Mutex<Vec<Message>>>,
}

impl FlowFixture {
    /// Deploy `workflow` on a fresh engine with the default (test) config.
    pub(crate) async fn deploy(workflow: &Workflow) -> Self {
        Self::deploy_with(workflow, &Config::default()).await
    }

    /// Deploy `workflow` with the cell's own config (a run bound, a tick).
    pub(crate) async fn deploy_with(workflow: &Workflow, config: &Config) -> Self {
        let engine = Engine::builder().set_config(config).start().await.unwrap();
        Self::finish(engine, workflow).await
    }

    async fn finish(engine: Engine, workflow: &Workflow) -> Self {
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
    pub(crate) async fn start(&mut self, vars: Vars) -> String {
        self.try_start(vars).await.unwrap()
    }

    /// Start the model, preserving a refusal (an unsupported model declaration,
    /// a duplicate pid): the cell that pins the refusal does not need an engine
    /// of its own.
    pub(crate) async fn try_start(&mut self, vars: Vars) -> crate::Result<String> {
        let pid = self.engine.runtime().start(&self.workflow, vars).await?;
        self.pid = pid.clone();
        Ok(pid)
    }

    /// Start with the cell's own pid (the `pid` option), answering the pid.
    pub(crate) async fn start_with_pid(&mut self, pid: &str, vars: Vars) -> crate::Result<String> {
        let mut vars = vars;
        vars.set(consts::PROCESS_ID, pid);
        let started = self.engine.runtime().start(&self.workflow, vars).await?;
        self.pid = started.clone();
        Ok(started)
    }

    /// Start and wait for the run's terminal lifecycle event.
    pub(crate) async fn run(&mut self, vars: Vars) -> Terminal {
        self.start(vars).await;
        self.wait_terminal().await
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn pid(&self) -> &str {
        &self.pid
    }

    /// Every task/act message the engine announced, in delivery order.
    pub(crate) fn messages(&self) -> Vec<Message> {
        self.messages.lock().clone()
    }

    /// The messages of the model node `nid`.
    pub(crate) fn messages_of(&self, nid: &str) -> Vec<Message> {
        self.messages
            .lock()
            .iter()
            .filter(|message| message.nid == nid)
            .cloned()
            .collect()
    }

    /// The latest message of `nid` in `state`, if it was announced.
    pub(crate) fn message(&self, nid: &str, state: MessageState) -> Option<Message> {
        self.messages
            .lock()
            .iter()
            .rev()
            .find(|message| message.nid == nid && message.state == state)
            .cloned()
    }

    /// Wait until `nid` announces `state` and answer that message.
    pub(crate) async fn wait_message(&self, nid: &str, state: MessageState) -> Message {
        let what = format!("{nid} to announce {state:?}");
        poll_until(&what, || async { self.message(nid, state).is_some() }).await;
        self.message(nid, state)
            .unwrap_or_else(|| panic!("{what}: message disappeared"))
    }

    /// Wait for the `created` message that names the act to address.
    pub(crate) async fn wait_created(&self, nid: &str) -> Created {
        self.wait_message(nid, MessageState::Created).await
    }

    /// Wait for another `created` instance of `nid` (a fan-out creates one per
    /// item under the same model node): `except` is the instance already seen.
    pub(crate) async fn wait_created_except(&self, nid: &str, except: &str) -> Created {
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
    pub(crate) async fn wait_messages(&self, nid: &str, count: usize) -> Vec<Message> {
        let what = format!("{count} messages of {nid}");
        poll_until(&what, || async { self.messages_of(nid).len() >= count }).await;
        self.messages_of(nid)
    }

    /// Every state `nid` announced, in order (the legacy per-act state trail).
    pub(crate) fn announced(&self, nid: &str) -> Vec<MessageState> {
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

    pub(crate) async fn next(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Next, Vars::new()).await
    }

    pub(crate) async fn submit(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Submit, Vars::new()).await
    }

    pub(crate) async fn skip(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Skip, Vars::new()).await
    }

    pub(crate) async fn abort(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Abort, Vars::new()).await
    }

    pub(crate) async fn fail(
        &self,
        pid: &str,
        tid: &str,
        code: &str,
        message: &str,
    ) -> crate::Result<()> {
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

    pub(crate) async fn back(&self, pid: &str, tid: &str, to: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Back, Vars::new().with("to", to))
            .await
    }

    pub(crate) async fn cancel(&self, pid: &str, tid: &str) -> crate::Result<()> {
        self.act(pid, tid, EventAction::Cancel, Vars::new()).await
    }

    /// Wait until the run reports a terminal lifecycle event.
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
    pub(crate) async fn wait_completed(&self) -> Message {
        let what = format!("run {} to complete", self.pid);
        poll_until(&what, || async { !self.completions.lock().is_empty() }).await;
        self.completions.lock().last().cloned().unwrap()
    }

    /// Wait for the run's error and answer its lifecycle message.
    pub(crate) async fn wait_error(&self) -> Message {
        let what = format!("run {} to error", self.pid);
        poll_until(&what, || async { !self.errors.lock().is_empty() }).await;
        self.errors.lock().last().cloned().unwrap()
    }

    /// The run's completion outputs (its model's `exposes`).
    pub(crate) fn outputs(&self) -> Vars {
        self.completions
            .lock()
            .last()
            .map(|message| message.outputs.clone())
            .unwrap_or_default()
    }

    /// The run's aggregate — the durable execution states.
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
    pub(crate) async fn wait_state(&self, nid: &str, state: TaskState) {
        let what = format!("{nid} to reach {state:?}");
        poll_until(&what, || async { self.state(nid).await == state }).await;
    }

    /// Wait until every execution is terminal.
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
    pub(crate) async fn rows(&self) -> Option<Arc<ProcRead>> {
        proc_row(&self.engine, &self.pid).await
    }

    /// The projected row of the model node `nid`.
    pub(crate) async fn row(&self, nid: &str) -> TaskRow {
        let rows = self.rows().await.expect("the run has projected rows");
        rows.task_by_nid(nid)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no task row for node '{nid}'"))
    }

    pub(crate) async fn close(self) {
        self.engine.close().await;
    }
}

/// A model whose every step is package-less (it self-completes), so a run
/// reaches its terminal state without a client.
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
pub(crate) fn irq_workflow(id: &str, step: &str, key: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(crate::model::Step {
        id: step.to_string(),
        name: step.to_string(),
        uses: Some(crate::utils::test::USES_IRQ.to_string()),
        params: Vars::new().with("key", key).to_value(),
        ..Default::default()
    });
    workflow
}
