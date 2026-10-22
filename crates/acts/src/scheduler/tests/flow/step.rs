//! Step cells migrated from the pre-kernel scheduler suite (`step/*` and the
//! `proc` while-loop cells): guards and loops, params, `uses` and the state a
//! step settles in. The catch cells live in `flow/catch.rs`.

use serial_test::serial;

use crate::{
    MessageState, Vars, Workflow,
    scheduler::core::model::task_graph::TaskState,
    utils::test::{USES_IRQ, USES_MSG, USES_SET},
};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

// ---------------------------------------------------------------------------
// Guards and loops
// ---------------------------------------------------------------------------

/// A step's own `if` is its entry guard: true runs it, false skips it and the
/// flow falls through to the next declared step (the legacy `step/if` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_if_guard_runs_or_skips_and_falls_through() {
    for (id, guard, expected) in [
        ("flow-step-if-true", "true", TaskState::Completed),
        ("flow-step-if-false", "false", TaskState::Skipped),
    ] {
        let mut workflow = Workflow::new()
            .with_id(id)
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| step.with_id("s2"));
        workflow.steps[0].r#if = Some(guard.to_string());
        let mut fixture = FlowFixture::deploy(&workflow).await;
        completed(&fixture.run(Vars::new()).await);
        assert_eq!(fixture.state("s1").await, expected, "{guard}");
        assert_eq!(
            fixture.state("s2").await,
            TaskState::Completed,
            "the flow continues past a skipped step"
        );
        fixture.close().await;
    }
}

/// A step with `while` re-executes its body while the guard holds, then falls
/// through to the next declared step (`examples/while`'s shape).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_while_attribute_loops_until_its_guard_fails() {
    let workflow = Workflow::from_yml(
        r#"
id: flow-while-attr
ver: 0.1.0
vars:
  - name: index
    value: 0
  - name: value
    value: 0
exposes:
  - name: result
    value: ${{ value }}
steps:
  - name: add
    id: add
    while: index < 3
    uses: acts.transform.set
    params:
      value: ${{ value + index }}
      index: ${{ index + 1 }}
  - name: end
    id: end
"#,
    )
    .unwrap();
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.outputs().get::<i32>("result"), Some(3));
    // The kernel completes a `while` execution once its guard closes (the
    // legacy engine parked a final skipped instance instead); the loop's
    // iterations are what the accumulated output proves.
    assert_eq!(fixture.state("add").await, TaskState::Completed);
    assert_eq!(fixture.state("end").await, TaskState::Completed);
    fixture.close().await;
}

/// A guard plus a self `next` is a bounded loop: the body runs while the guard
/// holds, and the failing evaluation is *skipped* without taking its own
/// `next`, so the flow falls through instead of looping forever.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_self_next_loop_stops_on_its_guard_and_falls_through() {
    let workflow = Workflow::new()
        .with_id("flow-step-self-next")
        .with_var("index", 0)
        .with_step(|step| {
            step.with_id("add")
                .with_if("index < 3")
                .with_next("add")
                .with_uses(USES_SET, Vars::new().with("index", "${{ index + 1 }}"))
        })
        .with_step(|step| step.with_id("end"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);

    let snapshot = fixture.snapshot().await;
    let mut generations: Vec<TaskState> = snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node.as_str() == "add")
        .map(|(_, task)| task.state)
        .collect();
    generations.sort_by_key(|state| format!("{state:?}"));
    assert_eq!(
        generations.len(),
        4,
        "three iterations plus the skipped one"
    );
    assert_eq!(
        generations
            .iter()
            .filter(|state| **state == TaskState::Completed)
            .count(),
        3
    );
    assert_eq!(
        generations
            .iter()
            .filter(|state| **state == TaskState::Skipped)
            .count(),
        1
    );
    assert_eq!(fixture.state("end").await, TaskState::Completed);
    fixture.close().await;
}

/// A step skipped by its guard must not take its explicit `next` (a self
/// `next` would otherwise re-schedule it forever): the declared chain wins.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_skipped_step_does_not_take_its_own_next() {
    let workflow = Workflow::new()
        .with_id("flow-step-if-false-next")
        .with_step(|step| step.with_id("s1").with_if("false").with_next("s1"))
        .with_step(|step| step.with_id("s2"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("s1").await, TaskState::Skipped);
    assert_eq!(fixture.state("s2").await, TaskState::Completed);
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Param values are resolved against the run's variables: literals stay as
/// declared, a whole `${{ }}` value keeps its type, and a string interpolates
/// its expressions (the legacy `act/params` and `step/params` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_params_resolve_literals_and_expressions() {
    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "literal",
            serde_json::json!("hello"),
            serde_json::json!("hello"),
        ),
        (
            "full",
            serde_json::json!("${{ \"hello\" }}"),
            serde_json::json!("hello"),
        ),
        (
            "partial",
            serde_json::json!("${{ \"hello\" }} world"),
            serde_json::json!("hello world"),
        ),
        (
            "typed-number",
            serde_json::json!("${{ n + 1 }}"),
            serde_json::json!(6),
        ),
        (
            "boolean",
            serde_json::json!("${{ a == \"hello\" }}"),
            serde_json::json!(true),
        ),
        (
            "arithmetic",
            serde_json::json!("${{ n + 1 }}"),
            serde_json::json!(6),
        ),
    ];
    for (id, data, expected) in cases {
        let mut workflow = Workflow::new()
            .with_id(&format!("flow-params-{id}"))
            .with_var("a", "hello")
            .with_var("b", "world")
            .with_var("n", 5);
        workflow.steps.push(crate::model::Step {
            id: "s1".to_string(),
            name: "s1".to_string(),
            uses: Some(USES_IRQ.to_string()),
            params: Vars::new()
                .with("key", "act")
                .with("data", data.clone())
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
        assert_eq!(data, expected, "{id}");
        fixture.next(&created.pid, &created.tid).await.unwrap();
        fixture.wait_completed().await;
        fixture.close().await;
    }
}

/// The durable model keeps the declared params on the node (`task.node().params`
/// in the legacy cells), independently of the resolved message payload.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_steps_declared_params_stay_on_its_node() {
    let workflow = Workflow::new()
        .with_id("flow-step-node-params")
        .with_step(|step| {
            step.with_id("s1").with_uses(
                USES_SET,
                Vars::new()
                    .with("action_name", "do_something")
                    .with("value", 42),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    let row = fixture.row("s1").await;
    let params = row.node().content.params();
    assert_eq!(params["action_name"], serde_json::json!("do_something"));
    assert_eq!(params["value"], serde_json::json!(42));
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// `uses` packages
// ---------------------------------------------------------------------------

/// Every `uses` step announces its act. `Msg`/`Set` self-complete, an `Irq`
/// waits until the client answers (`step/uses`); a `Msg` act's one
/// announcement is its `Completed` message.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_uses_packages_announce_and_settle() {
    // Msg: a self-completing act whose params ride its single delivery.
    let workflow = Workflow::new().with_id("flow-uses-msg").with_step(|step| {
        step.with_id("msg1").with_uses(
            USES_MSG,
            Vars::new().with("key", "m1").with("channel", "main"),
        )
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    let delivered = fixture.wait_message("msg1", MessageState::Completed).await;
    assert_eq!(delivered.r#type, "act");
    assert!(delivered.is_msg());
    assert_eq!(
        delivered
            .params()
            .unwrap()
            .get::<String>("channel")
            .as_deref(),
        Some("main")
    );
    assert_eq!(fixture.state("msg1").await, TaskState::Completed);
    fixture.close().await;

    // Set: a self-completing package, the step's own completion follows.
    let workflow = Workflow::new().with_id("flow-uses-set").with_step(|step| {
        step.with_id("set1")
            .with_uses(USES_SET, Vars::new().with("a", 1))
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    assert_eq!(fixture.state("set1").await, TaskState::Completed);
    fixture.close().await;

    // Irq: the act waits; the client's `next` completes it and the step.
    let workflow = Workflow::new().with_id("flow-uses-irq").with_step(|step| {
        step.with_id("irq1")
            .with_uses(USES_IRQ, Vars::new().with("key", "hold"))
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("irq1").await;
    assert_eq!(fixture.state("irq1").await, TaskState::Waiting);
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_message("irq1", MessageState::Completed).await;
    fixture.wait_completed().await;
    assert_eq!(fixture.state("irq1").await, TaskState::Completed);
    fixture.close().await;
}
