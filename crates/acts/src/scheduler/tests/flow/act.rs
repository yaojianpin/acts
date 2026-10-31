//! act cells migrated from the pre-kernel scheduler suite.
//!
//! The legacy `act/if`, `act/params` and `act/state` cells are kept for their
//! intent, not their plumbing: an inline `acts.core.block` act is materialized
//! by the kernel into a model child, so its own `if`, params and client
//! decisions are observed on that child's act message and execution rather
//! than through the legacy resident tree.
//!
//! The JavaScript cells of `act/params` are deliberately not ported as
//! working: the engine's evaluator is acts-expr over `${{ }}` templates, not a
//! JavaScript runtime, and an interpolation splices a string's raw value
//! (without JSON quoting). A whole-string JS program has no template at all
//! and is passed through verbatim (`fill_params`); that pass-through is pinned
//! below so nobody mistakes the legacy expectations for kernel behavior.

use crate::{
    ChannelOptions, MessageState, Principal, Step, Vars, Workflow,
    acl::UserPolicy,
    data::DeliveryStatus,
    event::EventAction,
    query::{Expr, Filter, Query},
    scheduler::{
        core::model::task_graph::TaskState,
        tests::{
            flow::{FlowFixture, Terminal, irq_workflow},
            reliability::poll_until,
        },
    },
    utils::{
        consts,
        test::{
            USES_ACTION, USES_BLOCK, USES_IRQ, USES_MSG, USES_PARALLEL, USES_SEQUENCE, USES_SET,
        },
    },
};
use serde_json::json;

use serial_test::serial;

fn assert_completed(terminal: Terminal) {
    match terminal {
        Terminal::Completed(_) => {}
        Terminal::Errored(message) => {
            panic!(
                "the run errored instead of completing: {:?} {:?}",
                message.state, message.outputs
            )
        }
    }
}

fn assert_errored(terminal: Terminal) {
    match terminal {
        Terminal::Errored(_) => {}
        Terminal::Completed(message) => {
            panic!("the run completed instead of erroring: {:?}", message.state)
        }
    }
}

/// One top-level block step over `acts`, mode `sequence`.
fn block_workflow(id: &str, acts: Vec<Step>) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(crate::model::Step {
        id: "step1".to_string(),
        name: "step1".to_string(),
        uses: Some(USES_BLOCK.to_string()),
        params: Vars::new()
            .with("mode", "sequence")
            .with("steps", acts)
            .to_value(),
        ..Default::default()
    });
    workflow
}

// ---------------------------------------------------------------------------
// Inline act `if`
// ---------------------------------------------------------------------------

/// `if: true` keeps the inline `msg` act: a `Msg` act is delivered once — its
/// `Completed` message, no `Created` before it — and its execution ends
/// `Completed` (`sch_act_if_true`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_true_runs_the_inline_act() {
    let workflow = block_workflow(
        "flow-act-if-true",
        vec![
            Step::new().with_uses(USES_SET, Vars::new().with("a", 10)),
            Step::new()
                .with_uses(USES_MSG, Vars::new())
                .with_id("msg1")
                .with_if("true"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    assert_completed(fixture.run(Vars::new()).await);

    let completed = fixture.wait_message("msg1", MessageState::Completed).await;
    assert_eq!(completed.r#type, "act");
    assert_eq!(completed.nid, "msg1");
    assert!(completed.is_msg());
    assert_eq!(fixture.state("msg1").await, TaskState::Completed);
    assert_eq!(
        fixture.announced("msg1"),
        vec![MessageState::Completed],
        "a Msg act is delivered exactly once, already completed"
    );
    fixture.close().await;
}

/// `if: false` skips the inline act without announcing Created: the skip is
/// its one message and its execution ends `Skipped` (`sch_act_if_false`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_false_skips_the_inline_act() {
    let workflow = block_workflow(
        "flow-act-if-false",
        vec![
            Step::new().with_uses(USES_SET, Vars::new().with("a", 10)),
            Step::new()
                .with_uses(USES_MSG, Vars::new())
                .with_id("msg1")
                .with_if("false"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    assert_completed(fixture.run(Vars::new()).await);

    let skipped = fixture.wait_message("msg1", MessageState::Skipped).await;
    assert_eq!(skipped.nid, "msg1");
    assert_eq!(fixture.state("msg1").await, TaskState::Skipped);
    assert_eq!(fixture.announced("msg1"), vec![MessageState::Skipped]);
    fixture.close().await;
}

/// `if: null` cannot decide the guard, so the act fails on its start operation
/// and the run errors (`sch_act_if_null_error`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_if_null_fails_the_inline_act() {
    let workflow = block_workflow(
        "flow-act-if-null",
        vec![
            Step::new().with_uses(USES_SET, Vars::new().with("a", 10)),
            Step::new()
                .with_uses(USES_MSG, Vars::new())
                .with_id("msg1")
                .with_if("null"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    assert_errored(fixture.run(Vars::new()).await);

    let failed = fixture.wait_message("msg1", MessageState::Error).await;
    assert_eq!(failed.nid, "msg1");
    assert_eq!(fixture.state("msg1").await, TaskState::Failed);
    assert_eq!(fixture.announced("msg1"), vec![MessageState::Error]);
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// An act's params resolve against the run's variables: literal, a whole
/// `${{ }}` value keeping its type, a partial-line interpolation, several
/// interpolated expressions in one string, and booleans
/// (`sch_act_params_*`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_resolve_literals_and_acts_expr_templates() {
    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        ("literal", json!("hello"), json!("hello")),
        ("full-line", json!("${{ \"hello\" }}"), json!("hello")),
        (
            "partial-line",
            json!("${{ \"hello\" }} world"),
            json!("hello world"),
        ),
        ("typed-number", json!("${{ 5 + 2 }}"), json!(7)),
        (
            "boolean-expression",
            json!("${{ a == \"hello\" }}"),
            json!(true),
        ),
        ("boolean-literal", json!(false), json!(false)),
    ];

    for (name, data, expected) in cases {
        let mut workflow = Workflow::new()
            .with_id(&format!("flow-act-params-{name}"))
            .with_var("a", "hello")
            .with_var("b", "world");
        workflow.steps.push(crate::model::Step {
            id: "s1".to_string(),
            name: "s1".to_string(),
            uses: Some(USES_IRQ.to_string()),
            params: Vars::new()
                .with("key", "act1")
                .with("data", data.clone())
                .to_value(),
            ..Default::default()
        });
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;

        let created = fixture.wait_created("s1").await;
        let resolved = created
            .params()
            .expect("the act params")
            .get::<serde_json::Value>("data")
            .expect("the data param");
        assert_eq!(resolved, expected, "{name}");

        fixture.next(&created.pid, &created.tid).await.unwrap();
        fixture.wait_message("s1", MessageState::Completed).await;
        fixture.close().await;
    }
}

/// A JavaScript `let` program is not an acts-expr expression and the kernel
/// has no JavaScript evaluator behind a step's params: a whole string that
/// contains no `${{ }}` template is passed through verbatim. The legacy
/// `sch_act_params_expr_multi_line`/`*_multi_expr_*` cells expected either a
/// JS evaluation or a JSON-quoted splice inside JS-like text; neither is the
/// kernel's acts-expr contract, so they are not pinned as working. This cell
/// pins the kernel's actual answer instead.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_params_javascript_is_not_evaluated() {
    let source = "let a = \"hello\";\nlet b = \"world\";\na + \" \" + b";
    let mut workflow = Workflow::new().with_id("flow-act-params-javascript");
    workflow.steps.push(crate::model::Step {
        id: "s1".to_string(),
        name: "s1".to_string(),
        uses: Some(USES_IRQ.to_string()),
        params: Vars::new()
            .with("key", "act1")
            .with("data", source)
            .to_value(),
        ..Default::default()
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    let data = created
        .params()
        .expect("the act params")
        .get::<serde_json::Value>("data")
        .expect("the data param");
    assert_eq!(data, json!(source));

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// State actions
// ---------------------------------------------------------------------------

/// `next` completes the waiting act: the message is `Completed` and the
/// execution terminal `Completed` (`sch_act_state_completed`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_action_next_completes_the_act() {
    let workflow = irq_workflow("flow-act-next", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_state("s1", TaskState::Waiting).await;

    let created = fixture.wait_created("s1").await;
    fixture.next(&created.pid, &created.tid).await.unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert!(completed.is_irq());
    assert_eq!(completed.nid, "s1");
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `submit` hands the act's work in: message and execution are `Submitted`
/// (`sch_act_state_submitted`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_action_submit_submits_the_act() {
    let workflow = irq_workflow("flow-act-submit", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    fixture.submit(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("s1", MessageState::Submitted).await;
    fixture.wait_state("s1", TaskState::Submitted).await;
    fixture.close().await;
}

/// `skip` closes the act as `Skipped` (`sch_act_state_skipped`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_action_skip_skips_the_act() {
    let workflow = irq_workflow("flow-act-skip", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    fixture.skip(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("s1", MessageState::Skipped).await;
    fixture.wait_state("s1", TaskState::Skipped).await;
    fixture.close().await;
}

/// `abort` closes the act as `Aborted` (`sch_act_state_aborted`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_action_abort_aborts_the_act() {
    let workflow = irq_workflow("flow-act-abort", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    fixture.abort(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("s1", MessageState::Aborted).await;
    fixture.wait_state("s1", TaskState::Aborted).await;
    fixture.close().await;
}

/// `error` fails the act: the message is `Error` and the execution `Failed`
/// (`sch_act_state_error`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_action_error_fails_the_act() {
    let workflow = irq_workflow("flow-act-error", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    fixture
        .fail(&created.pid, &created.tid, "err1", "boom")
        .await
        .unwrap();
    let failed = fixture.wait_message("s1", MessageState::Error).await;
    assert_eq!(
        failed.inputs.get::<String>(consts::ACT_ERR_CODE).as_deref(),
        Some("err1")
    );
    assert_eq!(
        failed
            .inputs
            .get::<String>(consts::ACT_ERR_MESSAGE)
            .as_deref(),
        Some("boom")
    );
    assert_eq!(fixture.state("s1").await, TaskState::Failed);
    fixture.wait_error().await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Rewinds
// ---------------------------------------------------------------------------

/// `back` on the second irq returns the flow to the first: `step2` announces
/// `Backed`, and `step1` is re-entered with a fresh waiting execution
/// (`sch_act_state_backed`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_back_rewinds_to_the_named_step() {
    let workflow = Workflow::new()
        .with_id("flow-act-back")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let step1 = fixture.wait_created("step1").await;
    fixture.next(&step1.pid, &step1.tid).await.unwrap();
    fixture.wait_message("step1", MessageState::Completed).await;
    let step2 = fixture.wait_created("step2").await;

    fixture.back(&step2.pid, &step2.tid, "step1").await.unwrap();
    fixture.wait_message("step2", MessageState::Backed).await;
    fixture.wait_state("step1", TaskState::Waiting).await;
    fixture.close().await;
}

/// `cancel` on the completed first irq closes it (`Cancelled` on its act
/// message) and re-enters it, while the second irq waits
/// (`sch_act_state_cancelled`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_cancel_rewinds_the_completed_act() {
    let workflow = Workflow::new()
        .with_id("flow-act-cancel")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let step1 = fixture.wait_created("step1").await;
    fixture.next(&step1.pid, &step1.tid).await.unwrap();
    fixture.wait_message("step1", MessageState::Completed).await;
    fixture.wait_created("step2").await;

    fixture.cancel(&step1.pid, &step1.tid).await.unwrap();
    fixture.wait_message("step1", MessageState::Cancelled).await;
    fixture.wait_state("step1", TaskState::Waiting).await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Waiting acts
// ---------------------------------------------------------------------------

/// An irq act does not complete on its own: while the client has not decided,
/// its execution is `Waiting` (`sch_act_state_interrupt`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_irq_waits_for_a_client() {
    let workflow = irq_workflow("flow-act-waiting", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    assert!(created.is_irq());
    fixture.wait_state("s1", TaskState::Waiting).await;
    assert!(fixture.message("s1", MessageState::Completed).is_none());

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Push and remove — the added and the dropped approver
// ---------------------------------------------------------------------------

/// `Push` adds an act under the waiting execution: the pushed act
/// announces itself with its own params and waits for its own client, and the
/// step's own completion waits for it — the added approver must act before
/// the step completes.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_push_adds_an_act_the_step_waits_for() {
    let workflow = irq_workflow("flow-act-push", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    fixture
        .act(
            &created.pid,
            &created.tid,
            EventAction::Push,
            Vars::new()
                .with("name", "act 2")
                .with("uses", crate::utils::test::USES_IRQ)
                .with("params", Vars::new().with("key", "act2")),
        )
        .await
        .unwrap();

    // The pushed act is a child execution of its own: it announces itself and
    // waits for its client, exactly like a declared act.
    let pushed = fixture.wait_created("s1#push1").await;
    assert_eq!(
        pushed
            .params()
            .expect("the pushed act's resolved params")
            .get::<String>("key")
            .as_deref(),
        Some("act2")
    );
    fixture.wait_state("s1#push1", TaskState::Waiting).await;
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);

    // The step's own answer parks it (the legacy `auto_complete` counts every
    // child task) — it completes only when the pushed act settles.
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_state("s1", TaskState::Running).await;
    assert_eq!(fixture.state("s1#push1").await, TaskState::Waiting);

    // Answering the pushed act completes the step and the run.
    fixture.next(&pushed.pid, &pushed.tid).await.unwrap();
    fixture.wait_state("s1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `Remove` drops an act: the removed act is decided `Removed` and the
/// flow continues — a step whose added approver is removed completes once its
/// own act was answered.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_remove_drops_the_added_act() {
    let workflow = irq_workflow("flow-act-remove", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    fixture
        .act(
            &created.pid,
            &created.tid,
            EventAction::Push,
            Vars::new()
                .with("uses", crate::utils::test::USES_IRQ)
                .with("params", Vars::new().with("key", "act2")),
        )
        .await
        .unwrap();
    let pushed = fixture.wait_created("s1#push1").await;

    // Answer the step, then remove the added approver instead of answering it.
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_state("s1", TaskState::Running).await;
    fixture
        .act(&pushed.pid, &pushed.tid, EventAction::Remove, Vars::new())
        .await
        .unwrap();

    fixture.wait_state("s1#push1", TaskState::Removed).await;
    // The state is the aggregate's; the message follows the outbox delivery.
    // Wait for it, then compare the trail after collapsing re-deliveries (the
    // wire is at-least-once).
    fixture
        .wait_message("s1#push1", MessageState::Removed)
        .await;
    let mut trail = fixture.announced("s1#push1");
    trail.dedup();
    assert_eq!(trail, vec![MessageState::Created, MessageState::Removed]);
    fixture.wait_state("s1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A plain `Remove` on a waiting act drops the act itself: the legacy walk
/// decided the task `Removed` and pushed the flow's next — the run completes
/// past it. A second remove is refused: the act is settled.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_remove_decides_a_waiting_act_removed_once() {
    let workflow = irq_workflow("flow-act-remove-self", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    fixture
        .act(&created.pid, &created.tid, EventAction::Remove, Vars::new())
        .await
        .unwrap();
    fixture.wait_state("s1", TaskState::Removed).await;
    fixture.wait_completed().await;

    let error = fixture
        .act(&created.pid, &created.tid, EventAction::Remove, Vars::new())
        .await
        .expect_err("a settled act takes no further decision");
    assert!(
        error.to_string().contains("ActionNotAllowed"),
        "duplicate remove: {error}"
    );
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Act inputs
// ---------------------------------------------------------------------------

/// An act message's inputs carry the `step` link (node id/name and the
/// instance's task id) and its resolved params, and a failure carries the
/// `ecode`/`message` facts in both `inputs` and `outputs` (`create_message`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_inputs_carry_the_step_link_and_error_facts() {
    let workflow = irq_workflow("flow-act-inputs", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    let step = created
        .inputs
        .get::<Vars>(consts::STEP_KEY)
        .expect("the act message carries its step link");
    assert_eq!(
        step.get::<String>(consts::STEP_NODE_ID).as_deref(),
        Some("s1")
    );
    assert_eq!(
        step.get::<String>(consts::STEP_NODE_NAME).as_deref(),
        Some("s1")
    );
    assert_eq!(
        step.get::<String>(consts::STEP_TASK_ID).as_deref(),
        Some(created.tid.as_str())
    );
    assert_eq!(
        created
            .params()
            .expect("the resolved act params")
            .get::<String>("key")
            .as_deref(),
        Some("act1")
    );

    fixture
        .fail(&created.pid, &created.tid, "err1", "boom")
        .await
        .unwrap();
    let failed = fixture.wait_message("s1", MessageState::Error).await;
    for vars in [&failed.inputs, &failed.outputs] {
        assert_eq!(
            vars.get::<String>(consts::ACT_ERR_CODE).as_deref(),
            Some("err1")
        );
        assert_eq!(
            vars.get::<String>(consts::ACT_ERR_MESSAGE).as_deref(),
            Some("boom")
        );
    }
    assert_eq!(fixture.state("s1").await, TaskState::Failed);
    fixture.wait_error().await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// The act's own tid — the client acts on the act, the owning step follows
// ---------------------------------------------------------------------------
//
// The wire (`Action`) is addressed by `pid` + `tid`, and since C5 the `tid`
// is one execution instance's wire — a container step's inline acts are
// executions of their own, each announcing itself with its own `tid`. The
// cells below serve client actions on the *act's* tid (not the step's) and
// pin the state the decision produces on the step that owns the act: the
// container join (or the failure walk) carries it up. A fan-out's items share
// one model node and take one generation each, so the per-instance tid is
// the only thing that tells them apart — the step's message cannot address
// them.

/// The execution of `node` the run holds at `generation`, as its wire tid —
/// what a client sends as the `Action`'s `tid` (C5).
fn wire_of(fixture: &FlowFixture, node: &str, generation: u64) -> String {
    let task_id = crate::scheduler::core::model::task_graph::TaskId::new(node, generation)
        .expect("a well-formed node id");
    task_id.wire(fixture.pid())
}

/// The execution one created message names: the model node it ran on. The
/// reverse lookup proves the message's `tid` is the act execution's address,
/// not the owning step's.
async fn named_execution(fixture: &FlowFixture, tid: &str) -> String {
    fixture
        .snapshot()
        .await
        .tasks
        .keys()
        .find(|id| id.wire(fixture.pid()) == tid)
        .map(|id| id.node.as_str().to_owned())
        .unwrap_or_else(|| panic!("no execution for tid {tid}"))
}

/// An irq act inside a `sequence` block is its own execution: its `Created`
/// message names the act's tid, the client answers that tid, and the step the
/// act belongs to completes only through the act's own state.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_irq_act_answers_on_the_acts_tid_and_the_step_follows() {
    let workflow = block_workflow(
        "flow-act-tid-irq",
        vec![
            Step::new()
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_id("act1"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("act1").await;
    assert_eq!(created.nid, "act1");
    // The addressed execution is the act's, not the owning step's.
    assert_eq!(
        named_execution(&fixture, &created.tid).await,
        "act1",
        "the act message names the act execution"
    );
    assert_ne!(created.tid, wire_of(&fixture, "step1", 0));

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("act1", MessageState::Completed).await;
    assert_eq!(fixture.state("act1").await, TaskState::Completed);
    // The act's state is what the owning step joined on.
    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// An `acts.core.action` act decides itself on its own tid: no client message
/// is served, the deferred decision rides the act's own operation commit, and
/// the step that owns it follows — and the flow continues past it (a stranded
/// `EffectUnknown` quarantine would never reach the second act).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_action_act_decides_itself_on_the_acts_tid() {
    let workflow = block_workflow(
        "flow-act-tid-action",
        vec![
            Step::new()
                .with_uses(USES_ACTION, Vars::new().with("action", "next"))
                .with_id("act2"),
            Step::new()
                .with_uses(USES_IRQ, Vars::new().with("key", "act3"))
                .with_id("act3"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    // The action act completed itself; the flow moved on to the irq act.
    fixture.wait_message("act2", MessageState::Completed).await;
    assert_eq!(fixture.state("act2").await, TaskState::Completed);

    // The run is live past the self-decided act: answer the irq on its tid.
    let created = fixture.wait_created("act3").await;
    assert_eq!(created.nid, "act3");
    assert_eq!(named_execution(&fixture, &created.tid).await, "act3");
    fixture.next(&created.pid, &created.tid).await.unwrap();

    fixture.wait_state("act3", TaskState::Completed).await;
    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.wait_settled().await;
    fixture.close().await;
}

/// A `block` act nested in a block is a container of its own: the client
/// answers the nested act's tid and the state climbs two owners — nested
/// block, then the step.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_block_act_passes_the_nested_acts_state_to_the_step() {
    let workflow = block_workflow(
        "flow-act-tid-block",
        vec![
            Step::new()
                .with_uses(
                    USES_BLOCK,
                    Vars::new().with("mode", "sequence").with(
                        "steps",
                        vec![
                            Step::new()
                                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                                .with_id("act1"),
                        ],
                    ),
                )
                .with_id("inner"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("act1").await;
    assert_eq!(named_execution(&fixture, &created.tid).await, "act1");
    fixture.next(&created.pid, &created.tid).await.unwrap();

    fixture.wait_state("act1", TaskState::Completed).await;
    fixture.wait_state("inner", TaskState::Completed).await;
    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A `sequence` act runs its template once per `in` item, one item after the
/// other: each item's act announces itself with its own tid (same node, next
/// generation), the client answers each in turn, and the step completes when
/// the last item's act settled.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_sequence_act_answers_each_items_act_on_its_tid() {
    let workflow = block_workflow(
        "flow-act-tid-sequence",
        vec![
            Step::new()
                .with_uses(
                    USES_SEQUENCE,
                    Vars::new().with("in", ["u1", "u2"]).with(
                        "steps",
                        vec![
                            Step::new()
                                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                                .with_id("act1"),
                        ],
                    ),
                )
                .with_id("seq"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let first = fixture.wait_created("act1").await;
    assert_eq!(named_execution(&fixture, &first.tid).await, "act1");
    fixture.next(&first.pid, &first.tid).await.unwrap();
    fixture.wait_message("act1", MessageState::Completed).await;

    // The second item's act is a new execution of the same node: its own tid.
    let second = fixture.wait_created_except("act1", &first.tid).await;
    assert_eq!(named_execution(&fixture, &second.tid).await, "act1");
    fixture.next(&second.pid, &second.tid).await.unwrap();
    fixture.wait_message("act1", MessageState::Completed).await;

    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A `parallel` act admits every item at once: both items' acts wait together,
/// the client answers each on its own tid, and the step joins when both
/// settled — either order.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_parallel_act_answers_both_items_acts_on_their_tids() {
    let workflow = block_workflow(
        "flow-act-tid-parallel",
        vec![
            Step::new()
                .with_uses(
                    USES_PARALLEL,
                    Vars::new().with("in", ["u1", "u2"]).with(
                        "steps",
                        vec![
                            Step::new()
                                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                                .with_id("act1"),
                        ],
                    ),
                )
                .with_id("par"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let first = fixture.wait_created("act1").await;
    let second = fixture.wait_created_except("act1", &first.tid).await;
    assert_eq!(named_execution(&fixture, &first.tid).await, "act1");
    assert_eq!(named_execution(&fixture, &second.tid).await, "act1");
    assert_ne!(first.tid, second.tid);

    fixture.next(&first.pid, &first.tid).await.unwrap();
    fixture.next(&second.pid, &second.tid).await.unwrap();

    fixture.wait_state("act1", TaskState::Completed).await;
    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `submit` on the act's tid hands the act's work in: the act is `Submitted`,
/// and the submitted act is exactly what the owning step joins on — the step
/// completes and the run ends.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_submit_on_the_acts_tid_joins_the_step() {
    let workflow = block_workflow(
        "flow-act-tid-submit",
        vec![
            Step::new()
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_id("act1"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("act1").await;
    fixture.submit(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("act1", MessageState::Submitted).await;
    assert_eq!(fixture.state("act1").await, TaskState::Submitted);
    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `skip` on the act's tid drops the act: the skip is terminal for the act,
/// and the owning step joins past it — the flow continues.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_skip_on_the_acts_tid_joins_the_step() {
    let workflow = block_workflow(
        "flow-act-tid-skip",
        vec![
            Step::new()
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_id("act1"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("act1").await;
    fixture.skip(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("act1", MessageState::Skipped).await;
    assert_eq!(fixture.state("act1").await, TaskState::Skipped);
    fixture.wait_state("step1", TaskState::Completed).await;
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `error` on the act's tid fails the act: the failure walks up the ownership
/// chain — the act's step ends `Failed` with the act's facts, and the run
/// errors (`emit_error` → `parent.set_err`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_error_on_the_acts_tid_fails_the_owning_step() {
    let workflow = block_workflow(
        "flow-act-tid-error",
        vec![
            Step::new()
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_id("act1"),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("act1").await;
    fixture
        .fail(&created.pid, &created.tid, "err1", "boom")
        .await
        .unwrap();

    let failed = fixture.wait_message("act1", MessageState::Error).await;
    assert_eq!(failed.nid, "act1");
    assert_eq!(fixture.state("act1").await, TaskState::Failed);
    fixture.wait_state("step1", TaskState::Failed).await;
    fixture.wait_error().await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Assignment (`assign.owner`) + per-task decision authorization
// ---------------------------------------------------------------------------

/// One top-level `Irq` act with its owner declaration.
fn assigned_workflow(id: &str, owner: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(id).with_var("reviewer", "alice");
    workflow.steps.push(
        Step::new()
            .with_id("s1")
            .with_name("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_assign(owner),
    );
    workflow
}

/// A non-unrestricted principal allowed to complete acts.
fn approved_user(name: &str) -> Principal {
    Principal::from_policy(&UserPolicy {
        name: name.to_string(),
        enabled: true,
        allow: vec!["act:complete".to_string()],
        ..Default::default()
    })
    .unwrap()
}

/// The persisted `__assignment` of the newest execution of `node`.
async fn assignment_of(fixture: &FlowFixture, node: &str) -> serde_json::Value {
    let snapshot = fixture.snapshot().await;
    let task = snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == node)
        .max_by_key(|id| id.generation)
        .cloned()
        .unwrap_or_else(|| panic!("no execution of {node}"));
    let raw = snapshot
        .variables
        .get(&(task, "__assignment".to_string()))
        .unwrap_or_else(|| panic!("{node} has no __assignment"));
    serde_json::from_str(raw).unwrap()
}

/// `assign.owner` resolves through the params path (`${{ reviewer }}`), is
/// persisted as the initial assignment, and rides the message projection:
/// top-level fields plus the `inputs.options` copy the channel globs read.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_resolves_and_projects_the_owner() {
    let workflow = assigned_workflow("flow-act-assign", "${{ reviewer }}");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    assert_eq!(created.owner.as_deref(), Some("alice"));
    assert_eq!(created.assignee.as_deref(), Some("alice"));
    assert_eq!(created.rev, 0);
    assert_eq!(created.delegator, None);
    assert_eq!(created.delegatee, None);
    let options = created.options().expect("the act options");
    assert_eq!(options.get::<String>("owner").as_deref(), Some("alice"));
    assert_eq!(options.get::<String>("assignee").as_deref(), Some("alice"));

    let assignment = assignment_of(&fixture, "s1").await;
    assert_eq!(assignment["owner"], json!("alice"));
    assert_eq!(assignment["assignee"], json!("alice"));
    assert_eq!(assignment["rev"], json!(0));
    assert_eq!(assignment["transferred_from"], json!(null));
    assert_eq!(assignment["updated_by"], json!(null));
    assert_eq!(assignment["updated_at"], json!(0));

    fixture.next(&created.pid, &created.tid).await.unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("alice"));
    assert_eq!(completed.assignee.as_deref(), Some("alice"));
    assert_eq!(completed.rev, 0);
    let options = completed.options().expect("the completed act options");
    assert_eq!(options.get::<String>("owner").as_deref(), Some("alice"));
    assert_eq!(options.get::<String>("assignee").as_deref(), Some("alice"));
    fixture.close().await;
}

/// An owner expression resolving to nothing fails the act: no unowned
/// approval is produced.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_empty_owner_fails_the_act() {
    let workflow = assigned_workflow("flow-act-assign-empty", "");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let failed = fixture.wait_message("s1", MessageState::Error).await;
    let message = failed
        .outputs
        .get::<String>(consts::ACT_ERR_MESSAGE)
        .unwrap_or_default();
    assert!(
        message.contains("assign.owner") && message.contains("s1"),
        "the failure must name the act and the declaration: {message}"
    );
    assert_eq!(fixture.state("s1").await, TaskState::Failed);
    fixture.wait_error().await;
    fixture.close().await;
}

/// `assign` is refused at act start on everything that is not a human
/// decision leaf; the failure names the node.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_is_refused_on_non_human_nodes() {
    let mut container = block_workflow(
        "flow-act-assign-container",
        vec![
            Step::new()
                .with_uses(USES_MSG, Vars::new())
                .with_id("inner"),
        ],
    );
    container.steps[0].assign = Some(crate::model::Assign {
        owner: Some("alice".to_string()),
        candidates: None,
    });

    let mut func = Workflow::new().with_id("flow-act-assign-func");
    func.steps.push(
        Step::new()
            .with_id("f1")
            .with_uses(USES_SET, Vars::new().with("a", 1))
            .with_assign("alice"),
    );

    let mut msg = Workflow::new().with_id("flow-act-assign-msg");
    msg.steps.push(
        Step::new()
            .with_id("m1")
            .with_uses(USES_MSG, Vars::new())
            .with_assign("alice"),
    );

    for (node, workflow) in [("step1", container), ("f1", func), ("m1", msg)] {
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let failed = fixture.wait_message(node, MessageState::Error).await;
        let message = failed
            .outputs
            .get::<String>(consts::ACT_ERR_MESSAGE)
            .unwrap_or_default();
        assert!(
            message.contains(node),
            "the refusal must name the node '{node}': {message}"
        );
        assert_eq!(fixture.state(node).await, TaskState::Failed);
        fixture.wait_error().await;
        fixture.close().await;
    }
}

/// A string owner may resolve to a list of ids: duplicates collapse and the
/// single result projects.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_resolves_a_deduplicated_owner() {
    let workflow = Workflow::new()
        .with_id("flow-act-assign-dedup")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("${{ reviewers }}")
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture
        .start(Vars::new().with("reviewers", json!(["u1", "u1"])))
        .await;

    let created = fixture.wait_created("s1").await;
    assert_eq!(created.owner.as_deref(), Some("u1"));
    assert_eq!(created.assignee.as_deref(), Some("u1"));
    fixture.close().await;
}

/// A multi-participant resolution cannot own one human act.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_rejects_multiple_participants() {
    let workflow = Workflow::new()
        .with_id("flow-act-assign-multi")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("${{ reviewers }}")
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture
        .start(Vars::new().with("reviewers", json!(["u1", "u2"])))
        .await;

    let failed = fixture.wait_message("s1", MessageState::Error).await;
    let message = failed
        .outputs
        .get::<String>(consts::ACT_ERR_MESSAGE)
        .unwrap_or_default();
    assert!(
        message.contains("exactly one owner") && message.contains("u1") && message.contains("u2"),
        "the failure must name the multi-owner resolution: {message}"
    );
    fixture.wait_error().await;
    fixture.close().await;
}

/// On a fresh run nothing has been decided, so the current actor is the
/// initiator sealed by the caller.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_current_actor_falls_back_to_the_initiator() {
    let workflow = Workflow::new()
        .with_id("flow-act-assign-actor")
        .with_rn("flow-act-assign-actor")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("${{ $actor }}")
        });
    let fixture = FlowFixture::deploy(&workflow).await;
    let starter = Principal::from_policy(&UserPolicy {
        name: "starter".to_string(),
        enabled: true,
        allow: vec!["model:deploy".to_string(), "proc:start".to_string()],
        patterns: vec!["flow-act-assign-actor".to_string()],
        ..Default::default()
    })
    .unwrap();
    fixture
        .engine()
        .executor(&starter)
        .proc()
        .start(&workflow.id, Vars::new().with("actor", "starter"))
        .await
        .unwrap();

    let created = fixture.wait_created("s1").await;
    assert_eq!(created.owner.as_deref(), Some("starter"));
    fixture.close().await;
}

/// `previous_approver` walks the decision history across top-level steps:
/// after u1 decides s1 and u2 decides s2, s3's owner is u1.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_previous_approver_follows_the_chain() {
    let workflow = Workflow::new()
        .with_id("flow-act-assign-previous")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("u1")
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("u2")
        })
        .with_step(|step| {
            step.with_id("s3")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("${{ $previous_approver() }}")
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let s1 = fixture.wait_created("s1").await;
    fixture
        .engine()
        .executor(&approved_user("u1"))
        .act()
        .complete(fixture.pid(), &s1.tid, Vars::new())
        .await
        .unwrap();
    let s2 = fixture.wait_created("s2").await;
    assert_eq!(s2.owner.as_deref(), Some("u2"));
    fixture
        .engine()
        .executor(&approved_user("u2"))
        .act()
        .complete(fixture.pid(), &s2.tid, Vars::new())
        .await
        .unwrap();

    let s3 = fixture.wait_created("s3").await;
    assert_eq!(
        s3.owner.as_deref(),
        Some("u1"),
        "the approver before u2 is u1"
    );
    fixture.close().await;
}

/// A declared assignment admits only its owner/assignee, and a decision's
/// revision must name the current one. A client's own owner/assignee fields
/// are discarded (the task state decides), and a rejected decision consumes
/// nothing.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_authorizes_owner_and_revision() {
    let workflow = assigned_workflow("flow-act-assign-auth", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    let stranger = approved_user("mallory");
    let refused = fixture
        .engine()
        .executor(&stranger)
        .act()
        .complete(
            &pid,
            &tid,
            Vars::new()
                .with("owner", "alice")
                .with("assignee", "alice")
                .with("rev", 0),
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER"),
        "a non-owner must be refused: {refused}"
    );

    let owner = approved_user("alice");
    let refused = fixture
        .engine()
        .executor(&owner)
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 9))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("EASSIGNMENT_CHANGED"),
        "a stale revision must be refused: {refused}"
    );

    fixture
        .engine()
        .executor(&owner)
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 0))
        .await
        .unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.close().await;
}

/// Without an assignment the task keeps the old `@execute` behavior, even for
/// an authenticated subject that is not any owner.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_without_assign_keeps_execute_behavior() {
    let workflow = irq_workflow("flow-act-assign-unassigned", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    let stranger = approved_user("mallory");
    fixture
        .engine()
        .executor(&stranger)
        .act()
        .complete(fixture.pid(), &created.tid, Vars::new())
        .await
        .unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.close().await;
}

/// ACL disabled (an unrestricted principal) keeps the old behavior: the
/// executor carries no subject, so the assignment is only a routing hint.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_assign_without_acl_keeps_old_behavior() {
    let workflow = assigned_workflow("flow-act-assign-no-acl", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .act()
        .complete(fixture.pid(), &created.tid, Vars::new())
        .await
        .unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Transfer (`act:transfer`)
// ---------------------------------------------------------------------------

/// A caller holding only the `assign` capability (not `@execute`).
fn assigner(name: &str) -> Principal {
    Principal::from_policy(&UserPolicy {
        name: name.to_string(),
        enabled: true,
        allow: vec!["@assign".to_string()],
        ..Default::default()
    })
    .unwrap()
}

/// Every delivery row of one execution instance.
async fn deliveries_of(fixture: &FlowFixture, tid: &str) -> Vec<crate::data::Delivery> {
    let q = Query::new().filter(Filter::and().expr(Expr::eq("tid", tid.to_string())));
    fixture
        .engine()
        .runtime()
        .store()
        .deliveries()
        .query(&q)
        .await
        .unwrap()
        .rows
}

async fn wait_delivery_present(fixture: &FlowFixture, tid: &str, msg_id: &str) {
    poll_until("the act delivery row", || async {
        deliveries_of(fixture, tid)
            .await
            .iter()
            .any(|delivery| delivery.msg_id == msg_id)
    })
    .await;
}

async fn wait_delivery_status(
    fixture: &FlowFixture,
    tid: &str,
    msg_id: &str,
    status: DeliveryStatus,
) -> crate::data::Delivery {
    poll_until("the act delivery to reach its status", || async {
        deliveries_of(fixture, tid)
            .await
            .iter()
            .any(|delivery| delivery.msg_id == msg_id && delivery.status == status)
    })
    .await;
    deliveries_of(fixture, tid)
        .await
        .into_iter()
        .find(|delivery| delivery.msg_id == msg_id && delivery.status == status)
        .unwrap_or_else(|| panic!("no {status} delivery of {msg_id}"))
}

/// The announced `type=assignment` notification.
async fn wait_assignment(fixture: &FlowFixture) -> crate::Message {
    poll_until("the assignment notification", || async {
        fixture
            .messages()
            .iter()
            .any(|message| message.r#type == "assignment")
    })
    .await;
    fixture
        .messages()
        .into_iter()
        .find(|message| message.r#type == "assignment")
        .expect("the assignment notification")
}

/// A transfer moves the owner without touching the act: the execution keeps
/// its id/state, the old delivery is superseded, the new owner receives the
/// `type=assignment` message, the old owner is refused and the new one can
/// decide. The durable action fact makes the transfer replayable.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_moves_the_owner_without_deciding_the_act() {
    let workflow = assigned_workflow("flow-act-transfer", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let ack = fixture.engine().channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    ack.on_message(|_| async {});
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    let before = fixture.snapshot().await;
    let task_id = before
        .tasks
        .keys()
        .find(|id| id.node.as_str() == "s1")
        .cloned()
        .expect("the act execution");
    let task_count = before.tasks.len();
    wait_delivery_present(&fixture, &tid, &created.id).await;

    // Alice is the owner: she may transfer without the `assign` capability.
    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(
            &pid,
            &tid,
            Vars::new()
                .with("to", "bob")
                .with("expect_rev", 0)
                .with("reason", "holiday"),
        )
        .await
        .unwrap();

    // The act itself is untouched: same instance, same state, no new task.
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);
    let after = fixture.snapshot().await;
    assert_eq!(after.tasks.len(), task_count, "a transfer creates no task");
    let unchanged = after.tasks.get(&task_id).expect("the same execution");
    assert_eq!(unchanged.state, TaskState::Waiting);

    // The assignment advanced, with the audit facts.
    let assignment = assignment_of(&fixture, "s1").await;
    assert_eq!(assignment["owner"], json!("bob"));
    assert_eq!(assignment["assignee"], json!("bob"));
    assert_eq!(assignment["rev"], json!(1));
    assert_eq!(assignment["transferred_from"], json!("alice"));
    assert_eq!(assignment["updated_by"], json!("alice"));
    assert!(assignment["updated_at"].as_i64().unwrap() > 0);

    // The transfer intent is a durable `__action` fact the operation replays.
    assert!(
        after.variables.iter().any(|((task, name), value)| {
            task == &task_id
                && name.starts_with("__action@")
                && value.contains("\"transfer\"")
                && value.contains("bob")
        }),
        "the transfer must be replayable from its action fact"
    );

    // The old owner's message is history; the new one carries the reference.
    let old = wait_delivery_status(&fixture, &tid, &created.id, DeliveryStatus::Superseded).await;
    assert_eq!(old.owner.as_deref(), Some("alice"));
    let notification = wait_assignment(&fixture).await;
    assert_eq!(notification.tid, tid);
    assert_eq!(notification.nid, "s1");
    assert_eq!(notification.owner.as_deref(), Some("bob"));
    assert_eq!(notification.assignee.as_deref(), Some("bob"));
    assert_eq!(notification.rev, 1);
    assert_eq!(
        notification.ref_msg_id.as_deref(),
        Some(created.id.as_str())
    );
    let options = notification.options().expect("the notification options");
    assert_eq!(options.get::<String>("owner").as_deref(), Some("bob"));
    assert_eq!(options.get::<String>("assignee").as_deref(), Some("bob"));
    // The original Created message stays as history.
    assert!(
        fixture
            .messages()
            .iter()
            .any(|message| message.id == created.id && message.state == MessageState::Created)
    );

    // The old owner can no longer decide; the new owner can, at revision 1.
    let refused = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER"),
        "the old owner must be refused: {refused}"
    );
    fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("bob"));
    assert_eq!(completed.assignee.as_deref(), Some("bob"));
    assert_eq!(completed.rev, 1);
    fixture.close().await;
}

/// A caller holding `@assign` may transfer an act it does not own; the
/// capability is independent of `@execute`.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_accepts_the_assign_capability() {
    let workflow = assigned_workflow("flow-act-transfer-assign", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    let manager = assigner("manager");
    assert!(
        manager.check("act:complete").is_err(),
        "the assign capability must not imply executing acts"
    );
    fixture
        .engine()
        .executor(&manager)
        .act()
        .transfer(
            fixture.pid(),
            &created.tid,
            Vars::new().with("to", "bob").with("expect_rev", 0),
        )
        .await
        .unwrap();
    let assignment = assignment_of(&fixture, "s1").await;
    assert_eq!(assignment["owner"], json!("bob"));
    assert_eq!(assignment["updated_by"], json!("manager"));
    fixture.close().await;
}

/// The wire action (`actions::apply_as`) authorizes the task's owner even
/// when the management policy grants it no `@execute` (or any other) command:
/// a transfer is authorized on the task, not only by a global grant.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_wire_action_authorizes_the_owner_without_execute() {
    let workflow = assigned_workflow("flow-act-transfer-wire", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let alice = Principal::from_policy(&UserPolicy {
        name: "alice".to_string(),
        enabled: true,
        ..Default::default()
    })
    .unwrap();

    let applied = crate::actions::apply_as(
        fixture.engine(),
        &alice,
        "act:transfer",
        Vars::new()
            .with("pid", fixture.pid())
            .with("tid", created.tid.clone())
            .with("to", "bob")
            .with("expect_rev", 0),
    )
    .await
    .expect("the owner may transfer without a global grant");
    // The act actions answer the executor's unit result as `null`, like
    // `act:complete`/`act:push` in the same dispatch table.
    assert_eq!(applied, json!(null));
    assert_eq!(assignment_of(&fixture, "s1").await["owner"], json!("bob"));
    fixture.close().await;
}

/// Every refusal is caller-correctable and consumes nothing: a stale
/// revision, a stranger, a missing target, a terminal act and an act with no
/// assignment.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_refuses_invalid_requests() {
    let workflow = assigned_workflow("flow-act-transfer-refuse", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    let stale = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(
            &pid,
            &tid,
            Vars::new().with("to", "bob").with("expect_rev", 9),
        )
        .await
        .unwrap_err();
    assert!(
        stale.to_string().contains("EASSIGNMENT_CHANGED"),
        "a stale revision must be refused: {stale}"
    );
    assert_eq!(assignment_of(&fixture, "s1").await["rev"], json!(0));
    assert!(
        fixture
            .messages()
            .iter()
            .all(|message| message.r#type != "assignment"),
        "a refused transfer must not announce an assignment"
    );

    let stranger = fixture
        .engine()
        .executor(&approved_user("mallory"))
        .act()
        .transfer(&pid, &tid, Vars::new().with("to", "bob"))
        .await
        .unwrap_err();
    assert!(
        stranger.to_string().contains("ENOTOWNER"),
        "a non-owner without assign must be refused: {stranger}"
    );

    let missing = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(&pid, &tid, Vars::new().with("to", " "))
        .await
        .unwrap_err();
    assert!(
        missing.to_string().contains("'to'"),
        "a missing target must be refused: {missing}"
    );

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 0))
        .await
        .unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    let terminal = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(&pid, &tid, Vars::new().with("to", "bob"))
        .await
        .unwrap_err();
    assert!(
        terminal.to_string().contains("not allowed to transfer"),
        "a terminal act must be refused: {terminal}"
    );
    fixture.close().await;

    // An act that declares no assignment has no owner to move.
    let workflow = irq_workflow("flow-act-transfer-unassigned", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let unassigned = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(fixture.pid(), &created.tid, Vars::new().with("to", "bob"))
        .await
        .unwrap_err();
    assert!(
        unassigned.to_string().contains("has no assignment"),
        "an unassigned act must be refused: {unassigned}"
    );
    fixture.close().await;
}

/// A transfer and a decision racing on one act: the command lane serializes
/// them, so exactly one wins and the loser gets its own refusal.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_and_complete_have_one_winner() {
    let workflow = assigned_workflow("flow-act-transfer-race", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let alice = approved_user("alice");

    let (transferred, completed) = tokio::join!(
        async {
            fixture
                .engine()
                .executor(&alice)
                .act()
                .transfer(
                    &pid,
                    &tid,
                    Vars::new().with("to", "bob").with("expect_rev", 0),
                )
                .await
        },
        async {
            fixture
                .engine()
                .executor(&alice)
                .act()
                .complete(&pid, &tid, Vars::new().with("rev", 0))
                .await
        }
    );
    match (transferred, completed) {
        // The decision won: the transfer sees a terminal act.
        (Err(error), Ok(())) => {
            assert!(
                error.to_string().contains("not allowed to transfer"),
                "the transfer lost to the decision: {error}"
            );
            assert_eq!(fixture.state("s1").await, TaskState::Completed);
        }
        // The transfer won: the old owner is no longer the owner.
        (Ok(()), Err(error)) => {
            assert!(
                error.to_string().contains("ENOTOWNER"),
                "the decision lost to the transfer: {error}"
            );
            assert_eq!(fixture.state("s1").await, TaskState::Waiting);
            assert_eq!(assignment_of(&fixture, "s1").await["owner"], json!("bob"));
        }
        (transferred, completed) => panic!(
            "exactly one of transfer/complete wins: transfer={transferred:?} complete={completed:?}"
        ),
    }
    fixture.close().await;
}

/// Restart: `__assignment`, the superseded delivery and the assignment
/// message are durable, and the new owner can decide the recovered act.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_survives_restart() {
    let workflow = assigned_workflow("flow-act-transfer-restart", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    let ack = fixture.engine().channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    ack.on_message(|_| async {});
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let store = fixture.engine().runtime().store().kv();
    wait_delivery_present(&fixture, &tid, &created.id).await;

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(
            &pid,
            &tid,
            Vars::new().with("to", "bob").with("expect_rev", 0),
        )
        .await
        .unwrap();
    let notification = wait_assignment(&fixture).await;
    let notification_id = notification.id.clone();
    wait_delivery_status(&fixture, &tid, &created.id, DeliveryStatus::Superseded).await;
    poll_until("the assignment message row", || async {
        fixture
            .engine()
            .runtime()
            .store()
            .messages()
            .find(&notification_id)
            .await
            .is_ok()
    })
    .await;
    fixture.close().await;

    let reloaded = crate::Engine::builder()
        .set_store(store)
        .start()
        .await
        .unwrap();
    let snapshot = reloaded
        .runtime()
        .workers()
        .snapshot(&pid)
        .await
        .unwrap()
        .expect("the recovered aggregate");
    let task = snapshot
        .tasks
        .keys()
        .find(|id| id.wire(&pid) == tid)
        .expect("the recovered execution");
    let raw = snapshot
        .variables
        .get(&(task.clone(), "__assignment".to_string()))
        .expect("the recovered assignment");
    let assignment: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(assignment["owner"], json!("bob"));
    assert_eq!(assignment["rev"], json!(1));
    assert!(
        reloaded
            .runtime()
            .store()
            .deliveries()
            .query(&Query::new().filter(Filter::and().expr(Expr::eq("tid", tid.clone()))))
            .await
            .unwrap()
            .rows
            .iter()
            .any(|delivery| delivery.msg_id == created.id
                && delivery.status == DeliveryStatus::Superseded),
        "the superseded delivery must survive the restart"
    );
    assert!(
        reloaded
            .runtime()
            .store()
            .messages()
            .find(&notification_id)
            .await
            .is_ok(),
        "the assignment message must survive the restart"
    );

    let refused = reloaded
        .executor(&approved_user("alice"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("ENOTOWNER"), "{refused}");
    reloaded
        .executor(&approved_user("bob"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap();
    poll_until("the recovered act to complete", || async {
        reloaded
            .runtime()
            .workers()
            .snapshot(&pid)
            .await
            .ok()
            .flatten()
            .is_some_and(|snapshot| {
                snapshot.tasks.iter().any(|(id, task)| {
                    id.node.as_str() == "s1" && task.state == TaskState::Completed
                })
            })
    })
    .await;
    reloaded.close().await;
}

// ---------------------------------------------------------------------------
// Delegate (`act:delegate` / `act:undelegate`)
// ---------------------------------------------------------------------------

/// The persisted `__delegation` of the newest execution of `node`, if it is
/// an active record (`null` reads as `None`).
async fn delegation_of(fixture: &FlowFixture, node: &str) -> Option<serde_json::Value> {
    let snapshot = fixture.snapshot().await;
    let task = snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == node)
        .max_by_key(|id| id.generation)
        .cloned()
        .unwrap_or_else(|| panic!("no execution of {node}"));
    let raw = snapshot
        .variables
        .get(&(task, "__delegation".to_string()))?;
    serde_json::from_str::<Option<serde_json::Value>>(raw)
        .ok()
        .flatten()
}

/// The announced `type=assignment` notification at `rev`.
async fn wait_assignment_at(fixture: &FlowFixture, rev: u64) -> crate::Message {
    let what = format!("the assignment notification at rev {rev}");
    poll_until(&what, || async {
        fixture
            .messages()
            .iter()
            .any(|message| message.r#type == "assignment" && message.rev == rev)
    })
    .await;
    fixture
        .messages()
        .into_iter()
        .find(|message| message.r#type == "assignment" && message.rev == rev)
        .unwrap_or_else(|| panic!("{what}"))
}

fn future_millis(offset: i64) -> i64 {
    crate::utils::time::time_millis() + offset
}

/// A delegation keeps the owner and the execution, records the delegation,
/// announces the delegatee and admits both the owner and the delegatee.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_admits_the_delegatee_and_projects_the_audit_fields() {
    let workflow = assigned_workflow("flow-act-delegate", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let before = fixture.snapshot().await;
    let task_count = before.tasks.len();
    let mut before_tasks: Vec<(String, u64)> = before
        .tasks
        .keys()
        .map(|id| (id.node.as_str().to_string(), id.generation))
        .collect();
    before_tasks.sort();
    let deadline = future_millis(600_000);

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new()
                .with("delegatee", "bob")
                .with("expires_at", deadline)
                .with("reason", "holiday")
                .with("rev", 0),
        )
        .await
        .unwrap();

    // The act itself is untouched: same instance, same state, no new task.
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);
    let after = fixture.snapshot().await;
    assert_eq!(
        after.tasks.len(),
        task_count,
        "a delegation creates no task"
    );
    let mut after_tasks: Vec<(String, u64)> = after
        .tasks
        .keys()
        .map(|id| (id.node.as_str().to_string(), id.generation))
        .collect();
    after_tasks.sort();
    assert_eq!(after_tasks, before_tasks, "no new generation");

    let assignment = assignment_of(&fixture, "s1").await;
    assert_eq!(assignment["owner"], json!("alice"));
    assert_eq!(assignment["assignee"], json!("bob"));
    assert_eq!(assignment["rev"], json!(1));
    assert_eq!(assignment["updated_by"], json!("alice"));
    assert!(assignment["updated_at"].as_i64().unwrap() > 0);

    let delegation = delegation_of(&fixture, "s1")
        .await
        .expect("the active delegation");
    assert_eq!(delegation["id"], json!("dg1"));
    assert_eq!(delegation["delegator"], json!("alice"));
    assert_eq!(delegation["delegatee"], json!("bob"));
    assert_eq!(delegation["granted_by"], json!("alice"));
    assert_eq!(delegation["expires_at"], json!(deadline));
    assert_eq!(delegation["reason"], json!("holiday"));

    let notification = wait_assignment_at(&fixture, 1).await;
    assert_eq!(notification.tid, tid);
    assert_eq!(notification.nid, "s1");
    assert_eq!(notification.owner.as_deref(), Some("alice"));
    assert_eq!(notification.assignee.as_deref(), Some("bob"));
    assert_eq!(notification.delegator.as_deref(), Some("alice"));
    assert_eq!(notification.delegatee.as_deref(), Some("bob"));
    assert_eq!(notification.rev, 1);
    assert_eq!(
        notification.ref_msg_id.as_deref(),
        Some(created.id.as_str())
    );
    let options = notification.options().expect("the notification options");
    assert_eq!(options.get::<String>("owner").as_deref(), Some("alice"));
    assert_eq!(options.get::<String>("assignee").as_deref(), Some("bob"));

    // The delegatee decides, and the Completed record carries the delegation.
    fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("alice"));
    assert_eq!(completed.assignee.as_deref(), Some("bob"));
    assert_eq!(completed.delegator.as_deref(), Some("alice"));
    assert_eq!(completed.delegatee.as_deref(), Some("bob"));
    assert_eq!(completed.rev, 1);
    let options = completed.options().expect("the completed act options");
    assert_eq!(options.get::<String>("owner").as_deref(), Some("alice"));
    assert_eq!(options.get::<String>("assignee").as_deref(), Some("bob"));
    fixture.close().await;
}

/// The owner may always decide; their decision records no delegation.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_keeps_the_owner_as_a_decider() {
    let workflow = assigned_workflow("flow-act-delegate-owner", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new()
                .with("delegatee", "bob")
                .with("expires_at", future_millis(600_000)),
        )
        .await
        .unwrap();
    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("alice"));
    assert_eq!(completed.assignee.as_deref(), Some("alice"));
    assert_eq!(completed.delegator, None);
    assert_eq!(completed.delegatee, None);
    assert_eq!(completed.rev, 1);
    fixture.close().await;
}

/// An expired delegation refuses the delegatee with the expiry fact; the
/// owner still decides. Expiry is checked at decision time, not by a sweep.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_refuses_the_delegatee_after_expiry() {
    let workflow = assigned_workflow("flow-act-delegate-expired", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let deadline = future_millis(-1_000);

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new()
                .with("delegatee", "bob")
                .with("expires_at", deadline)
                .with("rev", 0),
        )
        .await
        .unwrap();
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);

    let refused = fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER")
            && refused.to_string().contains("expired")
            && refused.to_string().contains("s1"),
        "the refusal must name the act and the expiry: {refused}"
    );
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap();
    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.close().await;
}

/// Undelegating restores the owner as the assignee and refuses the old
/// delegatee immediately.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_undelegate_revokes_the_delegatee() {
    let workflow = assigned_workflow("flow-act-undelegate", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new()
                .with("delegatee", "bob")
                .with("expires_at", future_millis(600_000))
                .with("rev", 0),
        )
        .await
        .unwrap();
    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .undelegate(&pid, &tid, Vars::new().with("rev", 1))
        .await
        .unwrap();

    let assignment = assignment_of(&fixture, "s1").await;
    assert_eq!(assignment["owner"], json!("alice"));
    assert_eq!(assignment["assignee"], json!("alice"));
    assert_eq!(assignment["rev"], json!(2));
    assert_eq!(assignment["updated_by"], json!("alice"));
    assert_eq!(
        delegation_of(&fixture, "s1").await,
        None,
        "the revoked delegation must read as absent"
    );

    let notification = wait_assignment_at(&fixture, 2).await;
    assert_eq!(notification.owner.as_deref(), Some("alice"));
    assert_eq!(notification.assignee.as_deref(), Some("alice"));
    assert_eq!(notification.delegator, None);
    assert_eq!(notification.delegatee, None);
    assert_eq!(notification.rev, 2);
    assert_eq!(
        notification.ref_msg_id.as_deref(),
        Some(created.id.as_str())
    );

    let refused = fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 2))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER"),
        "the revoked delegatee must be refused: {refused}"
    );
    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 2))
        .await
        .unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.assignee.as_deref(), Some("alice"));
    assert_eq!(completed.delegator, None);
    assert_eq!(completed.delegatee, None);
    fixture.close().await;
}

/// A transfer clears the old owner's delegation: the former delegatee cannot
/// act for the new owner, and the new owner can decide.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_clears_the_delegation() {
    let workflow = assigned_workflow("flow-act-transfer-delegation", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new()
                .with("delegatee", "bob")
                .with("expires_at", future_millis(600_000))
                .with("rev", 0),
        )
        .await
        .unwrap();
    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .transfer(
            &pid,
            &tid,
            Vars::new().with("to", "carol").with("expect_rev", 1),
        )
        .await
        .unwrap();

    let assignment = assignment_of(&fixture, "s1").await;
    assert_eq!(assignment["owner"], json!("carol"));
    assert_eq!(assignment["assignee"], json!("carol"));
    assert_eq!(assignment["rev"], json!(2));
    assert_eq!(
        delegation_of(&fixture, "s1").await,
        None,
        "a transfer must clear the old owner's delegation"
    );
    let notification = wait_assignment_at(&fixture, 2).await;
    assert_eq!(notification.owner.as_deref(), Some("carol"));
    assert_eq!(notification.delegator, None);
    assert_eq!(notification.delegatee, None);

    let refused = fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 2))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER"),
        "the old delegatee must not inherit the new owner's act: {refused}"
    );
    fixture
        .engine()
        .executor(&approved_user("carol"))
        .act()
        .complete(&pid, &tid, Vars::new().with("rev", 2))
        .await
        .unwrap();
    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.owner.as_deref(), Some("carol"));
    assert_eq!(completed.assignee.as_deref(), Some("carol"));
    assert_eq!(completed.delegator, None);
    assert_eq!(completed.delegatee, None);
    fixture.close().await;
}

/// Every delegation refusal is caller-correctable and consumes nothing: a
/// missing delegatee, a stranger, a stale revision, an unassigned act and an
/// act with no delegation to revoke.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_refuses_invalid_requests() {
    let workflow = assigned_workflow("flow-act-delegate-refuse", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());

    let missing = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(&pid, &tid, Vars::new().with("delegatee", " "))
        .await
        .unwrap_err();
    assert!(
        missing.to_string().contains("delegatee"),
        "a missing delegatee must be refused: {missing}"
    );

    let stranger = fixture
        .engine()
        .executor(&approved_user("mallory"))
        .act()
        .delegate(&pid, &tid, Vars::new().with("delegatee", "bob"))
        .await
        .unwrap_err();
    assert!(
        stranger.to_string().contains("ENOTOWNER"),
        "a non-owner without assign must be refused: {stranger}"
    );

    let stale = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new().with("delegatee", "bob").with("rev", 9),
        )
        .await
        .unwrap_err();
    assert!(
        stale.to_string().contains("EASSIGNMENT_CHANGED"),
        "a stale revision must be refused: {stale}"
    );

    let no_delegation = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .undelegate(&pid, &tid, Vars::new())
        .await
        .unwrap_err();
    assert!(
        no_delegation.to_string().contains("ENODELEGATION"),
        "nothing to revoke must be refused: {no_delegation}"
    );

    assert_eq!(assignment_of(&fixture, "s1").await["rev"], json!(0));
    assert!(
        fixture
            .messages()
            .iter()
            .all(|message| message.r#type != "assignment"),
        "refused delegations must not announce an assignment"
    );
    fixture.close().await;

    // An act that declares no assignment has no owner to delegate.
    let workflow = irq_workflow("flow-act-delegate-unassigned", "s1", "act1");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let unassigned = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            fixture.pid(),
            &created.tid,
            Vars::new().with("delegatee", "bob"),
        )
        .await
        .unwrap_err();
    assert!(
        unassigned.to_string().contains("has no assignment"),
        "an unassigned act must be refused: {unassigned}"
    );
    fixture.close().await;
}

/// A delegation and a decision racing on one act: the command lane and the
/// revision guard leave exactly one winner.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_and_complete_have_one_winner() {
    let workflow = assigned_workflow("flow-act-delegate-race", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let alice = approved_user("alice");

    let (delegated, completed) = tokio::join!(
        async {
            fixture
                .engine()
                .executor(&alice)
                .act()
                .delegate(
                    &pid,
                    &tid,
                    Vars::new().with("delegatee", "bob").with("rev", 0),
                )
                .await
        },
        async {
            fixture
                .engine()
                .executor(&alice)
                .act()
                .complete(&pid, &tid, Vars::new().with("rev", 0))
                .await
        }
    );
    match (delegated, completed) {
        (Err(error), Ok(())) => {
            assert!(
                error.to_string().contains("not allowed to delegate"),
                "the delegation lost to the decision: {error}"
            );
            assert_eq!(fixture.state("s1").await, TaskState::Completed);
        }
        (Ok(()), Err(error)) => {
            assert!(
                error.to_string().contains("EASSIGNMENT_CHANGED"),
                "the decision lost to the delegation: {error}"
            );
            assert_eq!(fixture.state("s1").await, TaskState::Waiting);
            assert_eq!(assignment_of(&fixture, "s1").await["rev"], json!(1));
        }
        (delegated, completed) => panic!(
            "exactly one of delegate/complete wins: delegate={delegated:?} complete={completed:?}"
        ),
    }
    fixture.close().await;
}

/// A transfer and a delegation racing on one act: exactly one wins.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_transfer_and_delegate_have_one_winner() {
    let workflow = assigned_workflow("flow-act-transfer-delegate-race", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let alice = approved_user("alice");

    let (transferred, delegated) = tokio::join!(
        async {
            fixture
                .engine()
                .executor(&alice)
                .act()
                .transfer(
                    &pid,
                    &tid,
                    Vars::new().with("to", "carol").with("expect_rev", 0),
                )
                .await
        },
        async {
            fixture
                .engine()
                .executor(&alice)
                .act()
                .delegate(
                    &pid,
                    &tid,
                    Vars::new().with("delegatee", "bob").with("rev", 0),
                )
                .await
        }
    );
    match (transferred, delegated) {
        (Ok(()), Err(error)) => {
            // The transfer moved the owner first: the old owner is no longer
            // an owner (ENOTOWNER), or the stale revision is named
            // (EASSIGNMENT_CHANGED) when the identity check still passes.
            assert!(
                error.to_string().contains("ENOTOWNER")
                    || error.to_string().contains("EASSIGNMENT_CHANGED"),
                "the delegation lost to the transfer: {error}"
            );
            assert_eq!(assignment_of(&fixture, "s1").await["owner"], json!("carol"));
            assert_eq!(delegation_of(&fixture, "s1").await, None);
        }
        (Err(error), Ok(())) => {
            assert!(
                error.to_string().contains("EASSIGNMENT_CHANGED"),
                "the transfer lost to the delegation: {error}"
            );
            let assignment = assignment_of(&fixture, "s1").await;
            assert_eq!(assignment["owner"], json!("alice"));
            assert_eq!(assignment["assignee"], json!("bob"));
            assert!(delegation_of(&fixture, "s1").await.is_some());
        }
        (transferred, delegated) => panic!(
            "exactly one of transfer/delegate wins: transfer={transferred:?} delegate={delegated:?}"
        ),
    }
    fixture.close().await;
}

/// Restart: the delegation, the assignment and the wire action survive, and
/// the recovered delegatee decides with the audit fields intact.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_survives_restart() {
    let workflow = assigned_workflow("flow-act-delegate-restart", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let (pid, tid) = (fixture.pid().to_string(), created.tid.clone());
    let store = fixture.engine().runtime().store().kv();
    let deadline = future_millis(600_000);

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .delegate(
            &pid,
            &tid,
            Vars::new()
                .with("delegatee", "bob")
                .with("expires_at", deadline)
                .with("reason", "holiday")
                .with("rev", 0),
        )
        .await
        .unwrap();
    wait_assignment_at(&fixture, 1).await;
    fixture.close().await;

    let reloaded = crate::Engine::builder()
        .set_store(store)
        .start()
        .await
        .unwrap();
    // A channel makes the reloaded engine persist the delivered messages
    // (delivery rows carry their canonical message row).
    let ack = reloaded.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    ack.on_message(|_| async {});
    let snapshot = reloaded
        .runtime()
        .workers()
        .snapshot(&pid)
        .await
        .unwrap()
        .expect("the recovered aggregate");
    let task = snapshot
        .tasks
        .keys()
        .find(|id| id.wire(&pid) == tid)
        .expect("the recovered execution");
    let raw = snapshot
        .variables
        .get(&(task.clone(), "__delegation".to_string()))
        .expect("the recovered delegation");
    let delegation: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(delegation["delegator"], json!("alice"));
    assert_eq!(delegation["delegatee"], json!("bob"));
    assert_eq!(delegation["expires_at"], json!(deadline));
    assert_eq!(delegation["reason"], json!("holiday"));

    // The wire action still admits the delegatee after the restart.
    let applied = crate::actions::apply_as(
        &reloaded,
        &approved_user("bob"),
        "act:complete",
        Vars::new()
            .with("pid", pid.clone())
            .with("tid", tid.clone())
            .with("rev", 1),
    )
    .await
    .expect("the recovered delegatee may decide");
    assert_eq!(applied, json!(null));

    poll_until("the recovered delegated act to complete", || async {
        reloaded
            .runtime()
            .workers()
            .snapshot(&pid)
            .await
            .ok()
            .flatten()
            .is_some_and(|snapshot| {
                snapshot.tasks.iter().any(|(id, task)| {
                    id.node.as_str() == "s1" && task.state == TaskState::Completed
                })
            })
    })
    .await;
    poll_until("the recovered Completed message", || async {
        reloaded
            .runtime()
            .store()
            .messages()
            .query(
                &Query::new()
                    .filter(Filter::and().expr(Expr::eq("tid", tid.clone())))
                    .limit(10),
            )
            .await
            .is_ok_and(|rows| {
                rows.rows.iter().any(|message| {
                    message.state == MessageState::Completed
                        && message.delegator.as_deref() == Some("alice")
                        && message.delegatee.as_deref() == Some("bob")
                })
            })
    })
    .await;
    reloaded.close().await;
}

/// The wire route (`act:delegate`/`act:undelegate`) reaches the same executor
/// and kernel authorization as the typed executor.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_delegate_wire_actions() {
    let workflow = assigned_workflow("flow-act-delegate-wire", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let alice = approved_user("alice");

    let applied = crate::actions::apply_as(
        fixture.engine(),
        &alice,
        "act:delegate",
        Vars::new()
            .with("pid", fixture.pid())
            .with("tid", created.tid.clone())
            .with("delegatee", "bob")
            .with("expires_at", future_millis(600_000))
            .with("reason", "holiday")
            .with("rev", 0),
    )
    .await
    .expect("the owner may delegate through the wire");
    assert_eq!(applied, json!(null));
    assert_eq!(assignment_of(&fixture, "s1").await["rev"], json!(1));
    assert_eq!(
        delegation_of(&fixture, "s1").await.expect("the delegation")["delegatee"],
        json!("bob")
    );

    let applied = crate::actions::apply_as(
        fixture.engine(),
        &alice,
        "act:undelegate",
        Vars::new()
            .with("pid", fixture.pid())
            .with("tid", created.tid.clone())
            .with("rev", 1),
    )
    .await
    .expect("the owner may revoke through the wire");
    assert_eq!(applied, json!(null));
    assert_eq!(assignment_of(&fixture, "s1").await["rev"], json!(2));
    assert_eq!(delegation_of(&fixture, "s1").await, None);
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// `act:push` with `assign`
// ---------------------------------------------------------------------------

/// A pushed human act carries its own `assign.owner`: it resolves for that
/// child and is independently authorized.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_push_with_assign_owns_the_child_independently() {
    let workflow = assigned_workflow("flow-act-push-assign", "alice");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;

    fixture
        .act(
            &created.pid,
            &created.tid,
            EventAction::Push,
            Vars::new()
                .with("uses", USES_IRQ)
                .with("params", Vars::new().with("key", "act2"))
                .with("assign", json!({ "owner": "bob" })),
        )
        .await
        .unwrap();
    let pushed = fixture.wait_created("s1#push1").await;
    assert_eq!(pushed.owner.as_deref(), Some("bob"));
    assert_eq!(pushed.assignee.as_deref(), Some("bob"));
    assert_eq!(pushed.rev, 0);
    let options = pushed.options().expect("the pushed act options");
    assert_eq!(options.get::<String>("owner").as_deref(), Some("bob"));

    let assignment = assignment_of(&fixture, "s1#push1").await;
    assert_eq!(assignment["owner"], json!("bob"));
    assert_eq!(assignment["rev"], json!(0));

    let refused = fixture
        .engine()
        .executor(&approved_user("mallory"))
        .act()
        .complete(&pushed.pid, &pushed.tid, Vars::new())
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER"),
        "the pushed act admits only its own owner: {refused}"
    );
    fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(&pushed.pid, &pushed.tid, Vars::new())
        .await
        .unwrap();
    fixture.wait_state("s1#push1", TaskState::Completed).await;

    fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(fixture.pid(), &created.tid, Vars::new().with("rev", 0))
        .await
        .unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A pushed non-human act (or an empty owner) fails at its start, naming the
/// pushed node — the engine never produces an unowned or non-human approval.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_push_assign_is_refused_on_non_human_or_empty() {
    for (uses, owner) in [(USES_SET, "bob"), (USES_IRQ, "")] {
        let workflow = irq_workflow("flow-act-push-assign-refuse", "s1", "act1");
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let created = fixture.wait_created("s1").await;
        fixture
            .act(
                &created.pid,
                &created.tid,
                EventAction::Push,
                Vars::new()
                    .with("uses", uses)
                    .with("params", Vars::new().with("a", 1))
                    .with("assign", json!({ "owner": owner })),
            )
            .await
            .unwrap();
        let failed = fixture.wait_message("s1#push1", MessageState::Error).await;
        let message = failed
            .outputs
            .get::<String>(consts::ACT_ERR_MESSAGE)
            .unwrap_or_default();
        assert!(
            message.contains("s1#push1"),
            "the refusal must name the pushed node: {message}"
        );
        assert_eq!(fixture.state("s1#push1").await, TaskState::Failed);
        fixture.close().await;
    }
}
