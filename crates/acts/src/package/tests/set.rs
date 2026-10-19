//! `acts.transform.set`: the act writes its params into its scope and they
//! propagate as outputs (`pack_set_*`).

use serde_json::json;
use serial_test::serial;

use crate::{Vars, Workflow, model::Step};

use super::{FlowFixture, USES_IRQ, USES_SET, one_step};

/// The set values are the step's data and the next node's inputs.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_writes_data_and_propagates_to_the_next_node() {
    let mut workflow = Workflow::new().with_id("pack-set-next");
    workflow.steps.push(Step {
        id: "s1".to_string(),
        name: "s1".to_string(),
        uses: Some(USES_SET.to_string()),
        params: Vars::new().with("a", 5).with("b", "text").to_value(),
        ..Default::default()
    });
    workflow.steps.push(Step {
        id: "s2".to_string(),
        name: "s2".to_string(),
        uses: Some(USES_IRQ.to_string()),
        params: Vars::new()
            .with("key", "act")
            .with("seen", "${{ a }}")
            .with("text", "${{ b }}")
            .to_value(),
        ..Default::default()
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s2").await;
    let params = created.params().unwrap();
    assert_eq!(params.get::<i32>("seen"), Some(5));
    assert_eq!(params.get::<String>("text").as_deref(), Some("text"));
    assert_eq!(fixture.row("s1").await.data().get::<i32>("a"), Some(5));

    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}

/// A set param resolves against the step's own declared vars.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_resolves_the_step_scope() {
    let mut workflow = Workflow::new().with_id("pack-set-scope");
    workflow.steps.push(Step {
        id: "s1".to_string(),
        name: "s1".to_string(),
        vars: vec![crate::Variant::create("b", json!("abc"))],
        uses: Some(USES_SET.to_string()),
        params: Vars::new().with("a", "${{ b }}").to_value(),
        ..Default::default()
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.run(Vars::new()).await;

    assert_eq!(
        fixture.row("s1").await.data().get::<String>("a").as_deref(),
        Some("abc")
    );
    fixture.close().await;
}

/// The set package is a plain transform: it has no side effect on the client
/// channel.
#[test]
fn pack_set_is_a_transform_package() {
    let _ = one_step("pack-set-schema", USES_SET, Vars::new().with("a", 1));
}
