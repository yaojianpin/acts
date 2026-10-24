//! Output-propagation cells: a node's outputs reach the next node (`next`)
//! and its parent (a branch/step scope), and an explicit `next` jumps the
//! chain. These are the data-flow half of the legacy `vars`/`step` cells.

use serial_test::serial;

use crate::{
    Vars, Workflow,
    scheduler::core::model::task_graph::TaskState,
    utils::test::{USES_IRQ, USES_SET},
};

use super::FlowFixture;

/// A step's outputs are its successor's inputs along the declared `next`
/// chain.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_outputs_flow_along_the_declared_next_chain() {
    let workflow = Workflow::new()
        .with_id("flow-outputs-next")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_SET, Vars::new().with("a", 5))
        })
        .with_step(|step| {
            step.with_id("s2").with_uses(
                USES_IRQ,
                Vars::new().with("key", "act").with("seen", "${{ a }}"),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s2").await;
    assert_eq!(
        created.params().unwrap().get::<i32>("seen"),
        Some(5),
        "s1's output is s2's input"
    );
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A bare step id reads that step's own durable data: the legacy `step1.a`
/// expression resolves through the shared aggregate, not through the acting
/// task's local variable map.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_bare_step_id_reads_a_previous_steps_data() {
    let workflow = Workflow::new()
        .with_id("flow-step-data")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_SET, Vars::new().with("a", 5))
        })
        .with_step(|step| {
            step.with_id("step2").with_uses(
                USES_IRQ,
                Vars::new()
                    .with("key", "act")
                    .with("seen", "${{ step1.a }}"),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("step2").await;
    assert_eq!(
        created.params().unwrap().get::<i32>("seen"),
        Some(5),
        "step1.a must read the completed step's own data"
    );
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A child's outputs flow up to its parent container, and from there to the
/// next node and the run's outputs (the legacy `branch_outputs` goal).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_outputs_flow_through_the_parent_to_the_next_node() {
    let workflow = Workflow::new()
        .with_id("flow-outputs-parent")
        .with_step(|step| {
            step.with_id("s1").with_branch(|branch| {
                branch.with_id("b1").with_if("true").with_step(|inner| {
                    inner
                        .with_id("i1")
                        .with_uses(USES_SET, Vars::new().with("x", 9))
                })
            })
        })
        .with_step(|step| {
            step.with_id("s2").with_uses(
                USES_IRQ,
                Vars::new().with("key", "act").with("seen", "${{ x }}"),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s2").await;
    assert_eq!(
        created.params().unwrap().get::<i32>("seen"),
        Some(9),
        "the branch child's output reaches the next top-level step"
    );

    fixture.next(&created.pid, &created.tid).await.unwrap();
    let completion = fixture.wait_completed().await;
    assert_eq!(
        fixture.state("b1").await,
        TaskState::Completed,
        "the parent container is settled"
    );
    assert_eq!(
        completion.outputs.get::<i32>("x"),
        Some(9),
        "the run's own scope carries the child's output"
    );
    fixture.close().await;
}

/// An explicit `next` (a backward/self reference, the only kind the model
/// accepts) re-enters the named node; each turn's output is what the next
/// node after the loop reads.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_outputs_flow_along_an_explicit_next() {
    let workflow = Workflow::new()
        .with_id("flow-outputs-explicit")
        .with_var("index", 0)
        .with_step(|step| {
            step.with_id("loop")
                .with_if("index < 2")
                .with_next("loop")
                .with_uses(
                    USES_SET,
                    Vars::new().with("a", 7).with("index", "${{ index + 1 }}"),
                )
        })
        .with_step(|step| {
            step.with_id("after").with_uses(
                USES_IRQ,
                Vars::new().with("key", "act").with("seen", "${{ a }}"),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("after").await;
    assert_eq!(
        created.params().unwrap().get::<i32>("seen"),
        Some(7),
        "the node after the loop reads the loop's output"
    );
    assert_eq!(
        fixture.state("loop").await,
        TaskState::Skipped,
        "the loop's final generation was skipped by its guard"
    );

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}
