//! An act that overran its task must be told to stop.
//!
//! The engine's only way to end a running act today is an action applied to
//! its task (`abort`, `cancel`, `skip`, `remove`, `next`, an external
//! `error`) or the engine shutting down. Both mark the task's state, but the
//! act itself keeps running to the end of whatever it started — a child
//! process, a request, a subscription — and holds its scheduler lane with it.
//! `Context::cancellation_token` is what those actions now reach, and these
//! tests are the contract: the token fires, the act's own error cannot undo
//! the state the action decided, and a shutdown fires the token too.

use crate::{
    ActError, ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Config, Engine,
    Result, Vars, Workflow,
    event::EventAction,
    scheduler::{Process, Task, TaskState},
    utils::longid,
};
use serde_json::json;
use serial_test::serial;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Longest a probe is waited for; the assert inside the wait reports which
/// report never arrived.
const PROBE_WAIT: Duration = Duration::from_secs(10);

/// The env key the probe acts report through: the process env is the one
/// handle a package and its test share.
const PROBE: &str = "probe";

/// Reports "running", then waits for the act's cancellation before reporting
/// "stopped". Its counterpart in the other act reports the same two facts and
/// then fails, to pin what an overridden task does with the act's error.
#[derive(Debug, Clone)]
struct ProbePackage;

/// Same as [`ProbePackage`], but returns an error once it is cancelled.
#[derive(Debug, Clone)]
struct FailingProbePackage;

#[async_trait::async_trait]
impl ActPackage for ProbePackage {
    fn definition() -> ActPackageDefinition {
        definition("test.scheduler.probe")
    }

    fn new(_: &Config) -> Result<Self> {
        Ok(Self)
    }

    async fn execute(
        &self,
        ctx: &crate::Context,
        _params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        ctx.set_env(PROBE, "running");
        ctx.cancellation_token().cancelled().await;
        ctx.set_env(PROBE, "stopped");
        Ok(None)
    }
}

#[async_trait::async_trait]
impl ActPackage for FailingProbePackage {
    fn definition() -> ActPackageDefinition {
        definition("test.scheduler.failing_probe")
    }

    fn new(_: &Config) -> Result<Self> {
        Ok(Self)
    }

    async fn execute(
        &self,
        ctx: &crate::Context,
        _params: &serde_json::Value,
    ) -> Result<Option<Vars>> {
        ctx.set_env(PROBE, "running");
        ctx.cancellation_token().cancelled().await;
        ctx.set_env(PROBE, "stopped");
        // A package that treats the cancellation as its own failure
        Err(ActError::Runtime("act stopped early".to_string()))
    }
}

fn definition(id: &'static str) -> ActPackageDefinition {
    ActPackageDefinition {
        id,
        name: "Probe",
        desc: "report when the act's cancellation token fires",
        icon: "",
        doc: "",
        version: "0.1.0",
        schema: json!({}),
        options: None,
        run_as: ActRunAs::Func,
        resources: Vec::new(),
        catalog: ActPackageCatalog::App,
    }
}

/// Poll the process env until the probe reports `value`.
async fn wait_probe(proc: &Arc<Process>, value: &str) {
    let deadline = Instant::now() + PROBE_WAIT;
    loop {
        if proc.with_env(|env| env.get::<String>(PROBE)).as_deref() == Some(value) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the probe act never reported '{value}' (last: {:?})",
            proc.with_env(|env| env.get::<String>(PROBE))
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The act task of the probe, once it is in flight. The enclosing step node
/// carries the same `uses` (a step `uses` is how a single-act step is
/// written), so the kind is what tells the two apart.
fn running_act(proc: &Arc<Process>, uses: &str) -> Arc<Task> {
    proc.find_tasks(|task| {
        task.is_kind(crate::NodeKind::Act) && task.is_uses(uses) && task.state().is_running()
    })
    .into_iter()
    .next()
    .expect("the probe act must be running")
}

/// `abort` on a running act reaches the act itself: its cancellation token
/// fires while it is still in flight, and the task keeps the state the action
/// decided.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_abort_reaches_the_running_act() {
    let engine = Engine::builder()
        .add_package::<ProbePackage>()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("probe-step")
            .with_uses("test.scheduler.probe", Vars::new())
    });
    let proc = rt.create_proc(&longid(), &workflow);
    rt.launch(&proc).await.unwrap();

    wait_probe(&proc, "running").await;
    let act = running_act(&proc, "test.scheduler.probe");

    rt.do_action2(proc.id(), &act.id, EventAction::Abort, Vars::new())
        .await
        .unwrap();

    // the act noticed: the abort did not have to wait for the act to finish
    wait_probe(&proc, "stopped").await;
    assert_eq!(act.state(), TaskState::Aborted);

    engine.close().await;
}

/// The same for `cancel`'s undo of a running path, and for the engine's own
/// shutdown: a closing engine must not leave an act running behind it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_shutdown_reaches_the_running_act() {
    let engine = Engine::builder()
        .add_package::<ProbePackage>()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("probe-step")
            .with_uses("test.scheduler.probe", Vars::new())
    });
    let proc = rt.create_proc(&longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    wait_probe(&proc, "running").await;

    engine.close().await;

    // the act gave its wait up rather than being abandoned with the engine.
    // `close` fires the token; the act's own task is scheduled after that, so
    // its report is waited for instead of assumed to have landed before the
    // close returned
    wait_probe(&proc, "stopped").await;
}

/// An act that fails *because* it was overridden does not overwrite the state
/// the action decided: `abort` stays `aborted` instead of turning into the
/// error the act reported on its way out.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_error_cannot_undo_an_override() {
    let engine = Engine::builder()
        .add_package::<FailingProbePackage>()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("probe-step")
            .with_uses("test.scheduler.failing_probe", Vars::new())
    });
    let proc = rt.create_proc(&longid(), &workflow);
    rt.launch(&proc).await.unwrap();

    wait_probe(&proc, "running").await;
    let act = running_act(&proc, "test.scheduler.failing_probe");

    rt.do_action2(proc.id(), &act.id, EventAction::Abort, Vars::new())
        .await
        .unwrap();
    wait_probe(&proc, "stopped").await;

    // wait for the run to end: the act's error lands before the process does
    let deadline = Instant::now() + PROBE_WAIT;
    while !proc.state().is_completed() {
        assert!(Instant::now() < deadline, "the aborted run never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert_eq!(act.state(), TaskState::Aborted);
    assert!(
        act.err().is_none(),
        "the overridden task must not carry the act's error: {:?}",
        act.err()
    );

    engine.close().await;
}
