//! Catch cells migrated from the pre-kernel scheduler suite (`step/catch`):
//! a step's `catches` handlers are selected by their own `if` against the
//! failure (`$ecode()`), an unmatched error resumes, and the handler's own
//! decision decides the caught step.

use serial_test::serial;

use crate::{
    Vars, Workflow,
    event::EventAction,
    scheduler::core::model::task_graph::TaskState,
    utils::test::{USES_IRQ, USES_MSG},
};

use super::FlowFixture;

fn catch_workflow(id: &str, handlers: &[(&str, Option<&str>)]) -> Workflow {
    let mut step = crate::model::Step::new()
        .with_id("s1")
        .with_uses(USES_IRQ, Vars::new().with("key", "act"));
    for (handler, guard) in handlers {
        let mut catch = crate::model::Step::new()
            .with_id(handler)
            .with_uses(USES_MSG, Vars::new().with("key", *handler));
        if let Some(guard) = guard {
            catch = catch.with_if(guard);
        }
        step.catches.push(catch);
    }
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(step);
    workflow.steps.push(crate::model::Step {
        id: "after".to_string(),
        name: "after".to_string(),
        ..Default::default()
    });
    workflow
}

/// A step whose catch handler waits for the client, so the handler's own
/// decision can be observed.
fn catch_action_workflow(id: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(id).with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act"))
            .with_catch(|catch| {
                catch
                    .with_id("handler")
                    .with_if(r#"$ecode() == "E1""#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "handler"))
            })
    });
    workflow.steps.push(crate::model::Step {
        id: "after".to_string(),
        name: "after".to_string(),
        ..Default::default()
    });
    workflow
}

/// A catch handler whose `if` matches the failure runs; one whose predicate
/// does not is skipped and does not catch it (`$ecode()`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_catch_is_selected_by_its_error_predicate() {
    let workflow = catch_workflow(
        "flow-catch-select",
        &[
            ("match", Some(r#"$ecode() == "E1""#)),
            ("miss", Some(r#"$ecode() == "E2""#)),
        ],
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let s1 = fixture.wait_created("s1").await;
    fixture.fail(&s1.pid, &s1.tid, "E1", "boom").await.unwrap();

    fixture.wait_state("match", TaskState::Completed).await;
    fixture.wait_state("miss", TaskState::Skipped).await;
    fixture.wait_completed().await;
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    assert_eq!(fixture.state("after").await, TaskState::Completed);
    fixture.close().await;
}

/// When every handler's predicate fails, the error resumes and the run fails —
/// the step is not silently completed.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_catch_no_matching_handler_resumes_the_error() {
    let workflow = catch_workflow("flow-catch-miss", &[("wrong", Some(r#"$ecode() == "E2""#))]);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let s1 = fixture.wait_created("s1").await;
    fixture.fail(&s1.pid, &s1.tid, "E1", "boom").await.unwrap();

    fixture.wait_state("wrong", TaskState::Skipped).await;
    fixture.wait_error().await;
    assert_eq!(fixture.state("s1").await, TaskState::Failed);
    fixture.close().await;
}

/// A handler without a predicate is unconditional: it catches the error and
/// the failed step completes.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_catch_unconditional_handler_handles_the_error() {
    let workflow = catch_workflow("flow-catch-default", &[("default", None)]);
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    let s1 = fixture.wait_created("s1").await;
    fixture.fail(&s1.pid, &s1.tid, "E1", "boom").await.unwrap();

    fixture.wait_state("default", TaskState::Completed).await;
    fixture.wait_completed().await;
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    assert_eq!(
        fixture.state("after").await,
        TaskState::Completed,
        "a caught error continues the chain"
    );
    fixture.close().await;
}

/// The handler's own decision decides the caught step: completing it completes
/// the step; failing it resumes the error; skipping it is still a caught error
/// (the legacy `catch_as_*` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_catch_handlers_decision_decides_the_caught_step() {
    for (id, action, handler_state, run_completed) in [
        ("next", EventAction::Next, TaskState::Completed, true),
        ("skip", EventAction::Skip, TaskState::Skipped, true),
        ("error", EventAction::Error, TaskState::Failed, false),
    ] {
        let workflow = catch_action_workflow(&format!("flow-catch-{id}"));
        let mut fixture = FlowFixture::deploy(&workflow).await;
        fixture.start(Vars::new()).await;
        let s1 = fixture.wait_created("s1").await;
        fixture.fail(&s1.pid, &s1.tid, "E1", "boom").await.unwrap();
        let handler = fixture.wait_created("handler").await;
        let options = if action == EventAction::Error {
            Vars::new()
                .with(crate::utils::consts::ACT_ERR_CODE, "E2")
                .with(crate::utils::consts::ACT_ERR_MESSAGE, "handler failed")
        } else {
            Vars::new()
        };
        fixture
            .act(&handler.pid, &handler.tid, action, options)
            .await
            .unwrap();
        fixture.wait_state("handler", handler_state).await;

        if run_completed {
            fixture.wait_completed().await;
            assert_eq!(fixture.state("s1").await, TaskState::Completed, "{id}");
        } else {
            fixture.wait_error().await;
            assert_eq!(fixture.state("s1").await, TaskState::Failed, "{id}");
        }
        fixture.close().await;
    }
}
