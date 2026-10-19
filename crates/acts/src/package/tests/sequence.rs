//! `acts.core.sequence`: one act instance per item of `in`, admitted one at a
//! time in declaration order (`pack_sequence_*`).

use serde_json::json;
use serial_test::serial;

use crate::{Act, MessageState, Vars};

use super::{FlowFixture, USES_SEQUENCE, one_step};

fn sequence_workflow(id: &str) -> crate::model::Workflow {
    one_step(
        id,
        USES_SEQUENCE,
        Vars::new().with("in", json!(["u1", "u2"])).with(
            "acts",
            vec![Act::irq(|act| {
                act.with_id("act1")
                    .with_params_vars(|vars| vars.with("key", "act1").with("value", "${{ value }}"))
            })],
        ),
    )
}

/// The first item waits for its client before the second is admitted; the
/// values arrive in declaration order.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_sequence_runs_its_items_in_order() {
    let workflow = sequence_workflow("pack-sequence-order");
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let first = fixture.wait_created("act1").await;
    assert_eq!(
        first.params().unwrap().get::<String>("value").as_deref(),
        Some("u1")
    );
    fixture.next(&first.pid, &first.tid).await.unwrap();
    fixture.wait_message("act1", MessageState::Completed).await;

    let second = fixture.wait_created_except("act1", &first.tid).await;
    assert_eq!(
        second.params().unwrap().get::<String>("value").as_deref(),
        Some("u2"),
        "the second item is admitted only after the first settles"
    );
    fixture.next(&second.pid, &second.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}
