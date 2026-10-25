//! Join and validation cells: the parallel container that decides by a join
//! rule instead of by "every child settled" — `any`, `partial(n)`/
//! `partial(n%)`, and the delegated `action` request the client answers. A
//! rule the container carries completes it early (the votes still out are
//! withdrawn), fails it once the threshold is unreachable, and delegates the
//! whole decision to a client verdict. The `sch_rules_*` cells cover the
//! step's own input validations (`rules`), decided when the act starts.

use serial_test::serial;

use crate::{
    JoinRule, Step, Vars, Workflow,
    scheduler::core::model::task_graph::TaskState,
    utils::test::{USES_BLOCK, USES_IRQ, USES_PARALLEL},
};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

fn errored(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Errored(_)),
        "the run must error, got {terminal:?}"
    );
}

/// A `acts.core.block{ mode: parallel }` step over one irq act per `key`,
/// with the join (or none) in the params.
fn block_step(join: Option<serde_json::Value>, keys: &[&str]) -> Step {
    let steps: Vec<serde_json::Value> = keys
        .iter()
        .map(|key| {
            serde_json::json!({
                "id": key,
                "uses": USES_IRQ,
                "params": { "key": key }
            })
        })
        .collect();
    let mut params = Vars::new().with("mode", "parallel");
    if let Some(rule) = join {
        params = params.with("join", rule);
    }
    Step::new()
        .with_id("s1")
        .with_uses(USES_BLOCK, params.with("steps", steps))
}

fn workflow_with(step: Step) -> Workflow {
    let mut workflow = Workflow::new().with_id("flow-rule");
    workflow.steps = vec![step];
    workflow
}

/// `any`: the first vote completes the container, and the vote still out is
/// withdrawn — skipped, not waited for.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_any_completes_on_the_first_vote_and_withdraws_the_rest() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!("any")),
        &["vote1", "vote2"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let _second = fixture.wait_created("vote2").await;

    fixture.next(&pid, &first.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    assert_eq!(fixture.state("vote2").await, TaskState::Skipped);
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// `any`: a failed vote is a vote against, not the container's death — the
/// surviving vote still decides (the legacy error walk would have killed the
/// whole chain).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_any_tolerates_a_failed_vote_until_a_completion_lands() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!("any")),
        &["vote1", "vote2"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let second = fixture.wait_created("vote2").await;

    fixture.fail(&pid, &first.tid, "e1", "no").await.unwrap();
    fixture.next(&pid, &second.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// `partial(2)`: the second of three votes completes the container; the
/// third vote is withdrawn.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_partial_count_completes_on_the_threshold() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!("partial(2)")),
        &["vote1", "vote2", "vote3"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let second = fixture.wait_created("vote2").await;
    let _third = fixture.wait_created("vote3").await;

    fixture.next(&pid, &first.tid).await.unwrap();
    fixture.next(&pid, &second.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    assert_eq!(fixture.state("vote3").await, TaskState::Skipped);
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// `partial(2)` over three votes: two failures leave one child that can
/// reach one completion of the two the rule needs — unreachable, so the
/// container fails and the run errors with it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_partial_fails_when_the_threshold_is_unreachable() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!("partial(2)")),
        &["vote1", "vote2", "vote3"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let second = fixture.wait_created("vote2").await;
    let _third = fixture.wait_created("vote3").await;

    // one failure still leaves two children that can reach the threshold
    fixture.fail(&pid, &first.tid, "e1", "no").await.unwrap();
    fixture.wait_state("s1#act0", TaskState::Failed).await;
    assert_eq!(fixture.state("s1").await, TaskState::Running);

    // the second failure sinks the reach below the threshold
    fixture.fail(&pid, &second.tid, "e1", "no").await.unwrap();

    fixture.wait_state("s1", TaskState::Failed).await;
    errored(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// No rule (the default `all`): a failed vote fails the run, exactly as the
/// engine always behaved.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_default_all_still_fails_the_run_on_a_failed_vote() {
    let workflow = workflow_with(block_step(None, &["vote1", "vote2"]));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;

    fixture.fail(&pid, &first.tid, "e1", "no").await.unwrap();

    fixture.wait_state("s1", TaskState::Failed).await;
    errored(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// `any` over the `acts.core.parallel{ in }` fan-out: the rule counts the
/// items, so one voter's answer completes the whole step and the other
/// item's vote is withdrawn.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_any_over_the_in_fanout_counts_the_items() {
    let mut step = Step::new().with_id("s1").with_uses(
        USES_PARALLEL,
        Vars::new()
            .with("in", "${{ users }}")
            .with("join", "any")
            .with(
                "steps",
                serde_json::json!([
                    { "id": "vote", "uses": USES_IRQ, "params": { "key": "vote" } }
                ]),
            ),
    );
    step.vars.push(crate::Variant {
        name: "users".into(),
        value: serde_json::json!(["a", "b"]),
        ..Default::default()
    });
    let workflow = workflow_with(step);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote").await;
    let _second = fixture.wait_created_except("vote", &first.tid).await;

    fixture.next(&pid, &first.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    // The completed instance may be either generation (the delivery order is
    // the fan-out's, not the generations'): one item completed, the other
    // was withdrawn.
    let snapshot = fixture.snapshot().await;
    let mut item_states: Vec<TaskState> = snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node.as_str() == "s1#item")
        .map(|(_, task)| task.state)
        .collect();
    item_states.sort_by_key(|state| state.as_str().to_string());
    assert_eq!(item_states, vec![TaskState::Completed, TaskState::Skipped]);
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A rule on the step itself (declared branches, no package): the same join,
/// one field on the container.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_any_on_declared_branches() {
    let workflow = Workflow::new()
        .with_id("flow-rule-branches")
        .with_step(|step| {
            step.with_id("s1")
                .with_join(JoinRule::Any)
                .with_branch(|branch| {
                    branch.with_id("b1").with_if("true").with_step(|inner| {
                        inner
                            .with_id("a1")
                            .with_uses(USES_IRQ, Vars::new().with("key", "a1"))
                    })
                })
                .with_branch(|branch| {
                    branch.with_id("b2").with_if("true").with_step(|inner| {
                        inner
                            .with_id("a2")
                            .with_uses(USES_IRQ, Vars::new().with("key", "a2"))
                    })
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("a1").await;
    let _second = fixture.wait_created("a2").await;

    fixture.next(&pid, &first.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    assert_eq!(fixture.state("a2").await, TaskState::Skipped);
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// The delegated rule asks on every settlement and the client's verdict
/// decides: completing the request completes the container and withdraws the
/// vote still out. The census carries the completed child's outputs — a
/// client's string input included, even though a completion stores it as
/// bare text — which is what lets a handler decide on who voted.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_delegated_verdict_completes_the_container() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!({ "action": "sign", "params": { "chair": true } })),
        &["vote1", "vote2"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let _second = fixture.wait_created("vote2").await;

    fixture
        .decide(
            &pid,
            &first.tid,
            crate::event::EventAction::Next,
            Vars::new().with("role", "chair"),
        )
        .await
        .unwrap();

    // The request carries the action key, the rule's params and the census.
    let request = fixture.wait_created("s1#rule1").await;
    assert_eq!(request.uses.as_deref(), Some("acts.core.join"));
    let params = request.inputs.get_value("params").expect("request params");
    assert_eq!(params["action"], serde_json::json!("sign"));
    assert_eq!(params["params"], serde_json::json!({ "chair": true }));
    // The census counts the container's own children — the branches the
    // block materialized — in no particular order: one completed (with the
    // vote's output), one still running.
    let outcomes = params["outcomes"].as_array().expect("census");
    assert_eq!(outcomes.len(), 2, "both branches are in the census");
    let completed_branch = outcomes
        .iter()
        .find(|outcome| outcome["state"] == serde_json::json!("completed"))
        .expect("one completed branch");
    assert_eq!(
        completed_branch["outputs"],
        serde_json::json!({ "role": "chair" }),
        "the completed child's outputs travel with the census"
    );
    let other = outcomes
        .iter()
        .find(|outcome| outcome["state"] == serde_json::json!("running"))
        .expect("one running branch");
    assert_eq!(other["outputs"], serde_json::json!({}));

    fixture.next(&pid, &request.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    assert_eq!(fixture.state("vote2").await, TaskState::Skipped);
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A failed verdict rejects the container with the answer's own error.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_delegated_fail_verdict_fails_the_container() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!({ "action": "sign" })),
        &["vote1", "vote2"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let _second = fixture.wait_created("vote2").await;

    fixture.next(&pid, &first.tid).await.unwrap();
    let request = fixture.wait_created("s1#rule1").await;
    fixture
        .fail(&pid, &request.tid, "rejected", "not approved")
        .await
        .unwrap();

    fixture.wait_state("s1", TaskState::Failed).await;
    assert_eq!(fixture.state("vote2").await, TaskState::Skipped);
    errored(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A skipped request defers: the next settlement asks again, and the new
/// request's verdict decides.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_delegated_skip_defers_to_the_next_settlement() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!({ "action": "sign" })),
        &["vote1", "vote2"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let first = fixture.wait_created("vote1").await;
    let second = fixture.wait_created("vote2").await;

    fixture.next(&pid, &first.tid).await.unwrap();
    let first_ask = fixture.wait_created("s1#rule1").await;
    fixture.skip(&pid, &first_ask.tid).await.unwrap();

    // The deferral settles the request; the container stays pending.
    fixture.wait_state("s1#rule1", TaskState::Skipped).await;
    assert_eq!(fixture.state("s1").await, TaskState::Running);

    fixture.next(&pid, &second.tid).await.unwrap();
    let second_ask = fixture.wait_created("s1#rule2").await;
    let outcomes = second_ask
        .inputs
        .get_value("params")
        .expect("request params")["outcomes"]
        .as_array()
        .expect("census")
        .clone();
    assert_eq!(outcomes.len(), 2, "the fresh census holds both votes");

    fixture.next(&pid, &second_ask.tid).await.unwrap();

    fixture.wait_state("s1", TaskState::Completed).await;
    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}

/// A join is refused where it cannot mean anything: on a sequence container,
/// and in a form that does not parse.
#[test]
fn sch_join_declaration_refusals() {
    let sequence_with_join = workflow_with(Step::new().with_id("s1").with_uses(
        "acts.core.sequence",
        Vars::new().with("join", "any").with(
            "steps",
            serde_json::json!([{ "uses": USES_IRQ, "params": { "key": "a" } }]),
        ),
    ));
    let error = sequence_with_join.valid().unwrap_err();
    assert!(error.to_string().contains("nothing to join"), "{error}");

    let malformed = workflow_with(block_step(
        Some(serde_json::json!("partial(0)")),
        &["vote1", "vote2"],
    ));
    let error = malformed.valid().unwrap_err();
    assert!(error.to_string().contains("invalid join"), "{error}");
}

/// `partial(3)` over two children: the fan-out knows the real count, so the
/// container fails loudly at its own dispatch — the standard failure shape —
/// instead of parking for a threshold nothing can reach.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_join_threshold_above_the_children_fails_loudly() {
    let workflow = workflow_with(block_step(
        Some(serde_json::json!("partial(3)")),
        &["vote1", "vote2"],
    ));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    fixture.wait_state("s1", TaskState::Failed).await;
    errored(&fixture.wait_terminal().await);
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Step `rules` — custom input validations
// ---------------------------------------------------------------------------

/// A violated rule fails the act at start, before it ever announces itself to
/// a client, with the rule's own message and the `validation_failed` code.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_fail_the_act_before_it_announces() {
    let workflow = Workflow::new()
        .with_id("flow-rules-fail")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_var("amount", 0)
                .with_validation(crate::ValidationRule {
                    r#if: "amount > 0".into(),
                    message: Some("amount must be positive".into()),
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
    let error = fixture.wait_error().await;
    assert!(
        error
            .outputs
            .to_string()
            .contains("amount must be positive")
            || format!("{error:?}").contains("amount must be positive"),
        "the failure carries the rule's message: {error:?}"
    );
    fixture.close().await;
}

/// A rule holds: the act starts normally. The expression reads the resolved
/// params by name, and the scope's variables too.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_rules_pass_on_the_resolved_params_and_scope() {
    let workflow = Workflow::new()
        .with_id("flow-rules-pass")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1").with("amount", 5))
                .with_var("limit", 10)
                .with_validation(crate::ValidationRule {
                    r#if: "amount > 0".into(),
                    message: None,
                })
                .with_validation(crate::ValidationRule {
                    r#if: "amount <= limit".into(),
                    message: None,
                })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let pid = fixture.start(Vars::new()).await;
    let act = fixture.wait_created("s1").await;

    fixture.next(&pid, &act.tid).await.unwrap();

    completed(&fixture.wait_terminal().await);
    fixture.close().await;
}
