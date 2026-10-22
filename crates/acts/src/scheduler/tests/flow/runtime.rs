//! Runtime cells migrated from the pre-kernel scheduler suite (`scher`,
//! `proc/start_single_shot` and `state`): start options, pid validation, the
//! one-run-per-pid rule, and the task-state vocabulary.

use serial_test::serial;

use crate::{MessageState, TaskState, Vars, utils, utils::consts};

use super::{FlowFixture, auto_workflow};

/// Start options are the run's inputs: they reach the root scope and the
/// root's `created` message (the legacy `sch_scher_start_with_vars`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_runtime_start_options_become_the_runs_inputs() {
    let workflow = auto_workflow("flow-start-vars", &["s1"]);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture
        .start_with_pid("chosenpid", Vars::new().with("a", 1).with("b", "text"))
        .await
        .unwrap();
    assert_eq!(pid, "chosenpid");

    let root = fixture.wait_message("$", MessageState::Created).await;
    assert_eq!(root.r#type, "workflow");
    assert_eq!(root.inputs.get::<i32>("a"), Some(1));
    assert_eq!(root.inputs.get::<String>("b").as_deref(), Some("text"));

    fixture.wait_completed().await;
    fixture.close().await;
}

/// A caller-supplied pid must be one safe key component: an empty pid and one
/// carrying the key separator are refused before anything durable exists (the
/// legacy `sch_scher_start_*_error` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_runtime_start_refuses_an_invalid_pid() {
    let workflow = auto_workflow("flow-start-invalid", &["s1"]);
    let mut fixture = FlowFixture::deploy(&workflow).await;

    let empty = fixture
        .start_with_pid("", Vars::new())
        .await
        .expect_err("an empty pid must be refused");
    assert!(
        empty.to_string().contains("cannot be empty"),
        "unexpected error: {empty}"
    );

    let separator = format!("proc{}123", consts::KEY_SEP);
    let separated = fixture
        .start_with_pid(&separator, Vars::new())
        .await
        .expect_err("a pid with the key separator must be refused");
    assert!(
        separated.to_string().contains("cannot contain"),
        "unexpected error: {separated}"
    );
    fixture.close().await;
}

/// A pid is a run's durable existence record: starting the same pid again is
/// refused, whether the first run is still in flight or already settled (the
/// legacy `sch_proc_start_single_shot`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_runtime_pid_runs_at_most_once() {
    let workflow = auto_workflow("flow-start-single", &["s1"]);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture
        .start_with_pid("singleshot", Vars::new())
        .await
        .unwrap();
    fixture.wait_completed().await;

    let duplicate = fixture
        .start_with_pid("singleshot", Vars::new())
        .await
        .expect_err("the pid is already taken");
    assert!(
        duplicate.to_string().contains("duplicated"),
        "unexpected error: {duplicate}"
    );
    fixture.close().await;
}

/// The wire vocabulary keeps its strings (the `sch_state_*` cells): the kernel
/// stores the same words in its projected rows, and every word parses back.
#[test]
fn sch_runtime_legacy_task_state_vocabulary_is_unchanged() {
    assert_eq!(TaskState::Completed.to_string(), "completed");
    assert_eq!(TaskState::Waiting.to_string(), "interrupted");
    assert_eq!(TaskState::Created.to_string(), "pending");
    assert_eq!(TaskState::parse("interrupted"), Some(TaskState::Waiting));
    assert_eq!(TaskState::parse("error"), Some(TaskState::Failed));
    assert_eq!(TaskState::parse("none"), None);

    assert!(TaskState::Completed.is_terminal());
    assert!(TaskState::Aborted.is_terminal());
    assert!(!TaskState::Running.is_terminal());
    assert_eq!(TaskState::Failed.as_str(), "error");
    assert_eq!(TaskState::Aborted.as_str(), "aborted");
    assert_eq!(TaskState::Running.as_str(), "running");
    assert_eq!(TaskState::Created.as_str(), "pending");
    assert_eq!(TaskState::Skipped.as_str(), "skipped");
    assert_eq!(TaskState::Waiting.as_str(), "interrupted");
    assert_eq!(TaskState::Ready.as_str(), "ready");
}

/// `states` reports every node's newest execution and `wait_settled` is the
/// "all terminal" barrier a cell uses before reading them.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_runtime_states_reports_every_node_after_the_run_settles() {
    let workflow = auto_workflow("flow-runtime-states", &["s1", "s2"]);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_settled().await;

    let states = fixture.states().await;
    for node in ["$", "s1", "s2"] {
        let state = states
            .iter()
            .find(|(name, _)| name == node)
            .unwrap_or_else(|| panic!("node '{node}' missing from {states:?}"))
            .1;
        assert_eq!(state, TaskState::Completed, "{node}");
    }
    fixture.close().await;
}

/// The pid is namespaced exactly once; the helper keeps the test's spelling of
/// a separator honest.
#[test]
fn sch_runtime_key_separator_is_still_the_one_validation_rejects() {
    assert_eq!(utils::consts::KEY_SEP, "-");
}
