//! Var cells migrated from the pre-kernel scheduler suite (`vars`, plus the
//! data assertions of `step/params` and `task`): the model's outputs, the
//! scope a step/branch/act reads, and the vars the read model carries.

use serial_test::serial;

use crate::{Vars, Workflow, scheduler::core::model::task_graph::TaskState, utils::test::USES_IRQ};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

/// A model's `exposes` is its output surface: an entry with a `value` is that
/// expression, an entry without one reads the variable of its name, and a
/// model without `exposes` reports its whole user scope (the legacy
/// `sch_vars_workflow_outputs_*` and `sch_vars_workflow_default_expose`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_workflow_outputs_follow_exposes_or_the_whole_scope() {
    let workflow = Workflow::from_yml(
        r#"
id: flow-vars-exposes
ver: 0.1.0
vars:
  - name: a
    value: 5
  - name: b
    value: 7
exposes:
  - name: value
    value: ${{ a + b }}
  - name: a
steps:
  - name: end
    id: end
"#,
    )
    .unwrap();
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    let outputs = fixture.outputs();
    assert_eq!(outputs.get::<i32>("value"), Some(12));
    assert_eq!(
        outputs.get::<i32>("a"),
        Some(5),
        "an expose without a value reads the variable"
    );
    assert!(
        outputs.get::<i32>("b").is_none(),
        "undeclared variables are not part of the output surface"
    );
    fixture.close().await;

    // No exposes: every user variable is an output.
    let workflow = Workflow::new()
        .with_id("flow-vars-default")
        .with_var("a", 5)
        .with_var("b", 7)
        .with_step(|step| step.with_id("end"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);
    let outputs = fixture.outputs();
    assert_eq!(outputs.get::<i32>("a"), Some(5));
    assert_eq!(outputs.get::<i32>("b"), Some(7));
    fixture.close().await;
}

/// A step's declared vars seed its own scope and reach the read model's data
/// row; the workflow vars stay readable up the chain (the legacy
/// `sch_step_params_with_vars` and `sch_vars_step_inputs`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_step_vars_reach_its_scope_and_its_row() {
    let workflow = Workflow::new()
        .with_id("flow-vars-step")
        .with_var("count", 10)
        .with_step(|step| {
            step.with_id("s1").with_var("local", 3).with_uses(
                USES_IRQ,
                Vars::new()
                    .with("key", "act")
                    .with("total", "${{ count + local }}"),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    assert_eq!(
        created.params().unwrap().get::<i32>("total"),
        Some(13),
        "workflow and step vars are both in scope"
    );

    let row = fixture.row("s1").await;
    assert_eq!(
        row.data().get::<i32>("local"),
        Some(3),
        "the step's own vars row carries its declared data"
    );
    // The workflow var is inherited, not copied into the step's row: it is
    // visible through the scope chain (`input_vars`, asserted above), which is
    // the kernel's durable split.

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    fixture.close().await;
}

/// A branch's declared vars are visible to its children while its own scope
/// stays under the step that declares it (the legacy `sch_vars_branch_*`
/// cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_vars_are_visible_to_its_children() {
    let workflow = Workflow::new()
        .with_id("flow-vars-branch")
        .with_var("root", 1)
        .with_step(|step| {
            step.with_id("s1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("true")
                    .with_var("branch_var", 4)
                    .with_step(|inner| {
                        inner.with_id("i1").with_uses(
                            USES_IRQ,
                            Vars::new()
                                .with("key", "act")
                                .with("total", "${{ root + branch_var }}"),
                        )
                    })
            })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("i1").await;
    assert_eq!(
        created.params().unwrap().get::<i32>("total"),
        Some(5),
        "the branch var and the workflow var both resolve"
    );
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    assert_eq!(fixture.state("b1").await, TaskState::Completed);
    fixture.close().await;
}

/// An act's declared options ride its payload as the client-facing options
/// set (`Message::options`), which is what a channel's option filter matches
/// (the legacy `sch_vars_act_options` and `sch_message_act_tag`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_options_ride_the_message() {
    let workflow = Workflow::new()
        .with_id("flow-vars-options")
        .with_step(|step| {
            step.with_id("s1")
                .with_option("tag", "business")
                .with_option("priority", 3)
                .with_uses(USES_IRQ, Vars::new().with("key", "act"))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("s1").await;
    let options = created.options().expect("the act options");
    assert_eq!(options.get::<String>("tag").as_deref(), Some("business"));
    assert_eq!(options.get::<i32>("priority"), Some(3));
    assert!(created.is_tag("business"));
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}
