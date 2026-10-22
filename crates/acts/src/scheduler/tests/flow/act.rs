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
    Act, MessageState, Vars, Workflow,
    event::EventAction,
    scheduler::{
        core::model::task_graph::TaskState,
        tests::flow::{FlowFixture, Terminal, irq_workflow},
    },
    utils::{consts, test::USES_IRQ},
};
use serde_json::json;

use serial_test::serial;

/// The container package whose inline `acts` the kernel materializes.
const USES_BLOCK: &str = "acts.core.block";

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
fn block_workflow(id: &str, acts: Vec<Act>) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(crate::model::Step {
        id: "step1".to_string(),
        name: "step1".to_string(),
        uses: Some(USES_BLOCK.to_string()),
        params: Vars::new()
            .with("mode", "sequence")
            .with("acts", acts)
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
            Act::set(Vars::new().with("a", 10)),
            Act::msg(|act| act.with_id("msg1").with_if("true")),
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
            Act::set(Vars::new().with("a", 10)),
            Act::msg(|act| act.with_id("msg1").with_if("false")),
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
            Act::set(Vars::new().with("a", 10)),
            Act::msg(|act| act.with_id("msg1").with_if("null")),
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
    assert_eq!(
        fixture.announced("s1#push1"),
        vec![MessageState::Created, MessageState::Removed]
    );
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
        vec![Act::irq(|act| {
            act.with_id("act1")
                .with_params_vars(|v| v.with("key", "act1"))
        })],
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
            Act::action(Vars::new().with("action", "next")).with_id("act2"),
            Act::irq(|act| {
                act.with_id("act3")
                    .with_params_vars(|v| v.with("key", "act3"))
            }),
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
            Act::block(Vars::new().with("mode", "sequence").with(
                "acts",
                vec![Act::irq(|act| {
                    act.with_id("act1")
                        .with_params_vars(|v| v.with("key", "act1"))
                })],
            ))
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
            Act::sequence(serde_json::json!({
                "in": ["u1", "u2"],
                "acts": [Act::irq(|act| {
                    act.with_id("act1")
                        .with_params_vars(|v| v.with("key", "act1"))
                })]
            }))
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
            Act::parallel(serde_json::json!({
                "in": ["u1", "u2"],
                "acts": [Act::irq(|act| {
                    act.with_id("act1")
                        .with_params_vars(|v| v.with("key", "act1"))
                })]
            }))
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
        vec![Act::irq(|act| {
            act.with_id("act1")
                .with_params_vars(|v| v.with("key", "act1"))
        })],
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
        vec![Act::irq(|act| {
            act.with_id("act1")
                .with_params_vars(|v| v.with("key", "act1"))
        })],
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
        vec![Act::irq(|act| {
            act.with_id("act1")
                .with_params_vars(|v| v.with("key", "act1"))
        })],
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
