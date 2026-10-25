//! `acts.core.parallel`: one act instance per item of `in`, all admitted
//! together, with the per-item `value`/`index` aliases (`pack_parallel_*`).

use serde_json::json;
use serial_test::serial;

use crate::{MessageState, Vars, Workflow, model::Step};

use super::{FlowFixture, USES_IRQ, USES_PARALLEL, one_step};

fn parallel_params(items: serde_json::Value) -> Vars {
    Vars::new().with("in", items).with(
        "steps",
        vec![
            Step::new()
                .with_uses(
                    USES_IRQ,
                    Vars::new()
                        .with("key", "act1")
                        .with("value", "${{ value }}")
                        .with("index", "${{ index }}"),
                )
                .with_id("act1"),
        ],
    )
}

fn parallel_workflow(id: &str, items: serde_json::Value) -> Workflow {
    one_step(id, USES_PARALLEL, parallel_params(items))
}

/// One instance per item; the per-item aliases are readable from the act's
/// params.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_parallel_creates_one_instance_per_item_with_value_and_index() {
    let workflow = parallel_workflow("pack-parallel-items", json!(["u1", "u2"]));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let messages = fixture.wait_messages("act1", 2).await;
    let created: Vec<_> = messages
        .into_iter()
        .filter(|message| message.state == MessageState::Created)
        .collect();
    assert_eq!(created.len(), 2, "one instance per item");
    let mut seen: Vec<(String, i32)> = created
        .iter()
        .map(|message| {
            let params = message.params().unwrap();
            (
                params.get::<String>("value").unwrap(),
                params.get::<i32>("index").unwrap(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(seen, [("u1".to_string(), 0), ("u2".to_string(), 1)]);

    for message in &created {
        fixture.next(&message.pid, &message.tid).await.unwrap();
    }
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `in` may name a variable that resolves to the collection.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_parallel_in_resolves_a_variable() {
    let mut workflow = Workflow::new()
        .with_id("pack-parallel-var")
        .with_var("items", json!(["u3"]));
    workflow.steps.push(Step {
        id: "s1".to_string(),
        name: "s1".to_string(),
        uses: Some(USES_PARALLEL.to_string()),
        params: parallel_params(json!("${{ items }}")).to_value(),
        ..Default::default()
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("act1").await;
    assert_eq!(
        created.params().unwrap().get::<String>("value").as_deref(),
        Some("u3")
    );
    fixture.next(&created.pid, &created.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}

/// `in` that does not resolve is refused: the act fails and the run errors
/// instead of silently running zero items.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_parallel_unresolved_in_errors() {
    let workflow = parallel_workflow("pack-parallel-missing", json!("${{ missing }}"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;
    fixture.wait_error().await;
    fixture.close().await;
}

/// The fixture's `USES_IRQ` is the same id the package declares; a regression
/// in the constant would be a compile error, not a silent skip.
#[test]
fn pack_parallel_uses_the_irq_package_id() {
    assert_eq!(USES_IRQ, "acts.core.irq");
}
