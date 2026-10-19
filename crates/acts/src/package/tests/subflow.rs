//! `acts.core.subflow`: the act starts the target model with the parent's
//! authority; an unknown target fails the act (`pack_subflow_*`).

use std::time::Duration;

use serde_json::json;
use serial_test::serial;

use crate::{Principal, Vars, Workflow};

use super::{FlowFixture, USES_SUBFLOW, one_step};

/// Starting a subflow emits the child's start lifecycle event.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_starts_the_target_model() {
    let main = one_step(
        "pack-subflow-main",
        USES_SUBFLOW,
        Vars::new().with("to", "pack-subflow-child"),
    );
    let mut fixture = FlowFixture::deploy(&main).await;
    let child = Workflow::new()
        .with_id("pack-subflow-child")
        .with_step(|step| step.with_id("child1"));
    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .model()
        .deploy(&child, None)
        .await
        .unwrap();

    let (tx, rx) = fixture.engine().signal(false).double();
    fixture.engine().channel().on_start(move |event| {
        let tx = tx.clone();
        async move {
            if event.mid == "pack-subflow-child" {
                tx.send(true);
            }
        }
    });

    fixture.start(Vars::new()).await;
    let started = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("the subflow child never started");
    assert!(started);
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A missing target is the package's own failure: the act fails the run.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_missing_target_errors() {
    let main = one_step(
        "pack-subflow-missing",
        USES_SUBFLOW,
        Vars::new().with("to", "missing-model"),
    );
    let mut fixture = FlowFixture::deploy(&main).await;
    fixture.start(Vars::new()).await;
    fixture.wait_error().await;
    fixture.close().await;
}

/// `to` is required; a subflow act without one is refused by the package
/// schema.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_requires_a_target() {
    let mut workflow = one_step("pack-subflow-no-target", USES_SUBFLOW, Vars::new());
    workflow.steps[0].params = json!({});
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_error().await;
    fixture.close().await;
}
