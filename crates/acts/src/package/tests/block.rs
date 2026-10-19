//! `acts.core.block`: inline acts materialized as children, in sequence or in
//! parallel (`pack_block_*`).

use serial_test::serial;

use crate::{Act, MessageState, Vars, scheduler::TaskState};

use super::{FlowFixture, USES_BLOCK, one_step};

fn msg_act(id: &str) -> Act {
    let id = id.to_string();
    Act::msg(move |act| {
        act.with_id(&id)
            .with_params_vars(|vars| vars.with("key", id.clone()))
    })
}

/// `mode: sequence` runs the inline acts in declaration order.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_block_sequence_runs_its_acts_in_order() {
    let workflow = one_step(
        "pack-block-sequence",
        USES_BLOCK,
        Vars::new()
            .with("mode", "sequence")
            .with("acts", vec![msg_act("m1"), msg_act("m2")]),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.run(Vars::new()).await;

    fixture.wait_messages("m1", 1).await;
    fixture.wait_messages("m2", 1).await;
    let order: Vec<String> = fixture
        .messages()
        .into_iter()
        .filter(|message| message.state == MessageState::Completed)
        .filter(|message| message.r#type == "act" && message.is_msg())
        .filter(|message| message.nid == "m1" || message.nid == "m2")
        .map(|message| message.nid)
        .collect();
    assert_eq!(order, ["m1", "m2"], "sequence order");
    fixture.close().await;
}

/// `mode: parallel` admits every inline act at once: both irq acts wait
/// together and complete together.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_block_parallel_admits_every_act() {
    let workflow = one_step(
        "pack-block-parallel",
        USES_BLOCK,
        Vars::new().with("mode", "parallel").with(
            "acts",
            vec![
                Act::irq(|act| act.with_id("a1").with_params_vars(|v| v.with("key", "a1"))),
                Act::irq(|act| act.with_id("a2").with_params_vars(|v| v.with("key", "a2"))),
            ],
        ),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let a1 = fixture.wait_created("a1").await;
    let a2 = fixture.wait_created("a2").await;
    assert_eq!(fixture.state("a1").await, TaskState::Waiting);
    assert_eq!(fixture.state("a2").await, TaskState::Waiting);

    fixture.next(&a1.pid, &a1.tid).await.unwrap();
    fixture.next(&a2.pid, &a2.tid).await.unwrap();
    fixture.wait_completed().await;
    fixture.close().await;
}
