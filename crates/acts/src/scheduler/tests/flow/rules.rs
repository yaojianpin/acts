// ---------------------------------------------------------------------------
// Step `rules` — custom input validations
// ---------------------------------------------------------------------------

use serial_test::serial;

use crate::{
    MessageState, Vars, Workflow, scheduler::core::model::task_graph::TaskState,
    utils::test::USES_IRQ,
};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

/// A triggered `error` rule fails the act at start, before it ever announces
/// itself to a client, with the rule's own message and the
/// `validation_failed` code.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_fail_the_act_before_it_announces() {
    let workflow = Workflow::new()
        .with_id("flow-rules-fail")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("amount", 0)
                .with_rule(crate::ValidationRule {
                    r#if: "amount <= 0".into(),
                    message: Some("amount must be positive".into()),
                    action: crate::event::EventAction::Error,
                    params: None,
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Failed).await;
    assert!(
        fixture
            .message("s1", crate::event::MessageState::Created)
            .is_none(),
        "the act must fail before it announces itself"
    );
    let error = fixture.wait_message("s1", MessageState::Error).await;
    assert_eq!(
        error
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("amount must be positive"),
        "the failed record carries the rule's message: {error:?}"
    );
    assert_eq!(
        error
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_CODE)
            .as_deref(),
        Some("validation_failed"),
        "the failed record carries the rule failure code: {error:?}"
    );
    fixture.wait_error().await;
    fixture.close().await;
}

/// No rule fires: the conditions read the resolved params by name and the
/// scope's variables, and all of them are false, so the act starts normally.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_pass_on_the_resolved_params_and_scope() {
    let workflow = Workflow::new()
        .with_id("flow-rules-pass")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1").with("amount", 5))
                .with_var("limit", 10)
                .with_rule(crate::ValidationRule {
                    r#if: "amount <= 0".into(),
                    message: None,
                    action: crate::event::EventAction::Error,
                    params: None,
                })
                .with_rule(crate::ValidationRule {
                    r#if: "amount > limit".into(),
                    message: None,
                    action: crate::event::EventAction::Error,
                    params: None,
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let act = fixture.wait_created("s1").await;

    fixture.next(&pid, &act.tid).await.unwrap();

    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// One post, one approver (the same person holds the manager and director posts). The
/// director approval's rule compares the previous approval's `approver`
/// (`$parent`) with the root's `director`; when they are the same person the
/// condition holds and its `skip` action drops that approval — no Created
/// message, the flow continues to the final step.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_skip_a_concurrent_post_approval() {
    let workflow = Workflow::new()
        .with_id("flow-rules-concurrent-post")
        .with_var("director", "u1")
        .with_step(|step| {
            step.with_id("manager_approval")
                .with_uses(USES_IRQ, Vars::new().with("key", "manager"))
                .with_var("approver", "u1")
        })
        .with_step(|step| {
            step.with_id("director_approval")
                .with_uses(USES_IRQ, Vars::new().with("key", "director"))
                .with_var("approver", "u1")
                .with_rule(crate::ValidationRule {
                    r#if: "$parent.approver == $root.director".into(),
                    message: Some("the director post is held by the manager".into()),
                    action: crate::event::EventAction::Skip,
                    params: None,
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;

    let manager = fixture.wait_created("manager_approval").await;
    fixture.next(&pid, &manager.tid).await.unwrap();

    fixture
        .wait_state("director_approval", TaskState::Skipped)
        .await;
    assert!(
        fixture
            .message("director_approval", MessageState::Created)
            .is_none(),
        "a skipped approval must not reach a client"
    );
    let skipped = fixture
        .wait_message("director_approval", MessageState::Skipped)
        .await;
    assert_eq!(
        skipped
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("the director post is held by the manager"),
        "the Skipped record tells the client why the approval was dropped: {skipped:?}"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// An auto-approval rule: the amount is under the approval limit, so the rule's
/// condition holds and its `next` action approves the act automatically — no
/// client decision and no manual `next` call.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_auto_approve_when_the_condition_holds() {
    let workflow = Workflow::new()
        .with_id("flow-rules-auto-approve")
        .with_var("amount", 100)
        .with_var("limit", 1000)
        .with_step(|step| {
            step.with_id("approval")
                .with_uses(USES_IRQ, Vars::new().with("key", "approval"))
                .with_rule(crate::ValidationRule {
                    r#if: "$root.amount <= $root.limit".into(),
                    message: None,
                    action: crate::event::EventAction::Next,
                    params: None,
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("approval", TaskState::Completed).await;
    assert!(
        fixture.message("approval", MessageState::Created).is_none(),
        "an auto-approved act must not ask a client"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// An `error` rule's own `params` name the failure facts: a string `code`
/// replaces `validation_failed` and a string `message` wins over the rule's
/// `message` field, so the failed record reports the rule's own error.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_error_params_name_the_failure() {
    let workflow = Workflow::new()
        .with_id("flow-rules-error-params")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("amount", 0)
                .with_rule(crate::ValidationRule {
                    r#if: "amount <= 0".into(),
                    message: Some("the message field".into()),
                    action: crate::event::EventAction::Error,
                    params: Some(
                        Vars::new()
                            .with("ecode", "E_AMOUNT")
                            .with("message", "amount must be positive"),
                    ),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let failed = fixture.wait_message("s1", MessageState::Error).await;
    assert_eq!(
        failed
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_CODE)
            .as_deref(),
        Some("E_AMOUNT"),
        "params.ecode names the failure code: {failed:?}"
    );
    assert_eq!(
        failed
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("amount must be positive"),
        "params.message wins over the rule's message field: {failed:?}"
    );
    fixture.wait_error().await;
    fixture.close().await;
}

/// Each `error` params key overrides only its own fact: `ecode` alone keeps
/// the rule's message as the failure text, and the default code is kept when
/// only `message` is declared.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_error_params_default_the_unspecified_fact() {
    let workflow = Workflow::new()
        .with_id("flow-rules-error-params-partial")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("amount", 0)
                .with_rule(crate::ValidationRule {
                    r#if: "amount <= 0".into(),
                    message: Some("the message field".into()),
                    action: crate::event::EventAction::Error,
                    params: Some(Vars::new().with(crate::utils::consts::ACT_ERR_CODE, "E_ONLY")),
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let failed = fixture.wait_message("s1", MessageState::Error).await;
    assert_eq!(
        failed
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_CODE)
            .as_deref(),
        Some("E_ONLY"),
        "params.ecode alone replaces the code: {failed:?}"
    );
    assert_eq!(
        failed
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("the message field"),
        "the rule's message stays when params declares none: {failed:?}"
    );
    fixture.wait_error().await;
    fixture.close().await;
}

/// A rule's own `params` execute its decision action: the `submit` rule
/// auto-submits the act with the rule's params as the decision's inputs, so
/// they land on the act's scope exactly like a client's own submission
/// inputs (the flag the example flip closes itself for the next turn). The
/// rule's `message` rides the same inputs, so the Submitted record tells the
/// client why it was decided.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_execute_the_decision_with_the_rule_params() {
    let workflow = Workflow::new()
        .with_id("flow-rules-params")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("auto_submit", true)
                .with_rule(crate::ValidationRule {
                    r#if: "auto_submit".into(),
                    message: Some("auto submitted on start".into()),
                    action: crate::event::EventAction::Submit,
                    params: Some(Vars::new().with("auto_submit", false)),
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Submitted).await;
    assert!(
        fixture.message("s1", MessageState::Created).is_none(),
        "a rule-submitted act must not ask a client"
    );
    let submitted = fixture.wait_message("s1", MessageState::Submitted).await;
    assert_eq!(
        submitted.outputs.get::<bool>("auto_submit"),
        Some(false),
        "the rule's params land on the act's record outputs: {submitted:?}"
    );
    assert_eq!(
        submitted
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("auto submitted on start"),
        "the rule's message rides the decision's record: {submitted:?}"
    );
    let snapshot = fixture.snapshot().await;
    let landed = snapshot
        .variables
        .iter()
        .find(|((task, name), _)| task.node.as_str() == "s1" && name == "auto_submit")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        landed,
        Some("false"),
        "the rule's params land on the act's scope as the decision's inputs"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A rule-submitted act's data folds into the workflow root like a completion,
/// and a rewind re-enters the step from its declared scope: the root-level
/// `auto_submit` flag is false after the first submit, so the re-entered
/// `prepare` generation reads false and waits for a client instead of
/// auto-submitting again.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_submitted_data_updates_the_root_and_survives_a_rewind() {
    let workflow = Workflow::new()
        .with_id("flow-rules-submit-rewind")
        .with_var("auto_submit", true)
        .with_step(|step| {
            step.with_id("prepare")
                .with_uses(USES_IRQ, Vars::new().with("key", "prepare"))
                .with_rule(crate::ValidationRule {
                    r#if: "auto_submit".into(),
                    message: None,
                    action: crate::event::EventAction::Submit,
                    params: Some(Vars::new().with("auto_submit", false)),
                })
        })
        .with_step(|step| {
            step.with_id("next")
                .with_uses(USES_IRQ, Vars::new().with("key", "next"))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // The first prepare auto-submits; its decision's `auto_submit: false`
    // folds into the parent root.
    fixture.wait_state("prepare", TaskState::Submitted).await;
    let next = fixture.wait_created("next").await;
    let snapshot = fixture.snapshot().await;
    let root_flag = snapshot
        .variables
        .iter()
        .find(|((task, name), _)| task.node.as_str() == "$" && name == "auto_submit")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        root_flag,
        Some("false"),
        "the submitted act's data folds into the workflow root"
    );

    // Back to prepare: the fresh generation starts from its declared vars
    // (none), reads the root's false, and waits.
    fixture.back(&next.pid, &next.tid, "prepare").await.unwrap();
    fixture.wait_message("next", MessageState::Backed).await;
    fixture.wait_state("prepare", TaskState::Waiting).await;
    let reentered = fixture.wait_created("prepare").await;
    assert_eq!(
        reentered.pid, next.pid,
        "the re-entered prepare announces itself as a fresh act"
    );
    assert_eq!(
        fixture
            .messages_of("prepare")
            .iter()
            .filter(|message| message.state == MessageState::Submitted)
            .count(),
        1,
        "the re-entered prepare must not auto-submit a second time"
    );
    let snapshot = fixture.snapshot().await;
    let root_flag = snapshot
        .variables
        .iter()
        .find(|((task, name), _)| task.node.as_str() == "$" && name == "auto_submit")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        root_flag,
        Some("false"),
        "the rewind leaves the root flag untouched"
    );
    fixture.close().await;
}

/// A rule without `params` decides exactly as before: the auto-completion
/// carries no inputs, and the act's scope keeps the declared variable.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_decide_without_params_by_default() {
    let workflow = Workflow::new()
        .with_id("flow-rules-no-params")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("auto_submit", true)
                .with_rule(crate::ValidationRule {
                    r#if: "auto_submit".into(),
                    message: None,
                    action: crate::event::EventAction::Submit,
                    params: None,
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Submitted).await;
    let snapshot = fixture.snapshot().await;
    let landed = snapshot
        .variables
        .iter()
        .find(|((task, name), _)| task.node.as_str() == "s1" && name == "auto_submit")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        landed,
        Some("true"),
        "a rule without params writes nothing on top of the scope"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// An explicit `params.message` wins over the rule's `message` field on a
/// decision: the terminal record reports the params' own text.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_decision_params_message_wins() {
    let workflow = Workflow::new()
        .with_id("flow-rules-decision-message")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("flag", true)
                .with_rule(crate::ValidationRule {
                    r#if: "flag".into(),
                    message: Some("the rule text".into()),
                    action: crate::event::EventAction::Next,
                    params: Some(
                        Vars::new().with(crate::utils::consts::ACT_ERR_MESSAGE, "the params text"),
                    ),
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let completed_action = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(
        completed_action
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("the params text"),
        "params.message wins over the rule's message field: {completed_action:?}"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A `skip` rule without a `message` reports no reason: the Skipped record
/// exists (the client still sees the terminal state) and its outputs carry no
/// `message` — the same shape a guard skip has.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_skip_without_a_message_reports_no_reason() {
    let workflow = Workflow::new()
        .with_id("flow-rules-skip-no-message")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("drop", true)
                .with_rule(crate::ValidationRule {
                    r#if: "drop".into(),
                    message: None,
                    action: crate::event::EventAction::Skip,
                    params: None,
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Skipped).await;
    let skipped = fixture.wait_message("s1", MessageState::Skipped).await;
    assert!(
        skipped
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .is_none(),
        "a skip rule without a message reports no reason: {skipped:?}"
    );
    assert!(
        fixture.message("s1", MessageState::Created).is_none(),
        "a rule-skipped act must not ask a client"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A skip rule's `params.message` wins over its `message` field as the
/// Skipped record's reason.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_skip_params_message_wins() {
    let workflow = Workflow::new()
        .with_id("flow-rules-skip-params-message")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("drop", true)
                .with_rule(crate::ValidationRule {
                    r#if: "drop".into(),
                    message: Some("the rule text".into()),
                    action: crate::event::EventAction::Skip,
                    params: Some(
                        Vars::new().with(crate::utils::consts::ACT_ERR_MESSAGE, "the params text"),
                    ),
                })
        })
        .with_step(|step| step.with_id("final"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Skipped).await;
    let skipped = fixture.wait_message("s1", MessageState::Skipped).await;
    assert_eq!(
        skipped
            .outputs
            .get::<String>(crate::utils::consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("the params text"),
        "params.message wins over the rule's message field: {skipped:?}"
    );
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}
