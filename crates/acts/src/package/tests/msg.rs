//! `acts.core.msg`: the act announces a message and completes on its own
//! (`pack_msg_*`).

use serial_test::serial;

use crate::{MessageState, Vars, scheduler::TaskState};

use super::{FlowFixture, USES_MSG, one_step};

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_msg_emits_its_params_and_completes() {
    let workflow = one_step(
        "pack-msg",
        USES_MSG,
        Vars::new()
            .with("key", "m1")
            .with("channel", "main")
            .with("a", 1),
    );
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_message("s1", MessageState::Created).await;
    assert!(created.is_msg());
    assert_eq!(created.r#type, "act");
    let params = created.params().expect("the message params");
    assert_eq!(params.get::<String>("key").as_deref(), Some("m1"));
    assert_eq!(params.get::<String>("channel").as_deref(), Some("main"));
    assert_eq!(params.get::<i32>("a"), Some(1));

    fixture.wait_message("s1", MessageState::Completed).await;
    fixture.wait_completed().await;
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    fixture.close().await;
}
