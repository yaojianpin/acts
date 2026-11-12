//! `acts.core.msg`: the act is delivered once — its `Completed` message
//! carries the params, typed `notice`, with the computed params as its
//! `outputs` — and completes on its own (`pack_msg_*`).

use serial_test::serial;

use crate::{MessageState, Vars, Workflow, scheduler::TaskState};

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

    // The single delivery: already `Completed`, typed `notice` — the type
    // word is the package's `Msg` run-bound, not the declaring step's level.
    // The params ride `inputs` as they do on a `created` message, and they
    // complete as the message's `outputs` (no `exposes`: the whole bag).
    let delivered = fixture.wait_message("s1", MessageState::Completed).await;
    assert!(delivered.is_msg());
    assert_eq!(delivered.r#type, "notice");
    let params = delivered.params().expect("the message params");
    assert_eq!(params.get::<String>("key").as_deref(), Some("m1"));
    assert_eq!(params.get::<String>("channel").as_deref(), Some("main"));
    assert_eq!(params.get::<i32>("a"), Some(1));
    assert_eq!(
        delivered.outputs.get::<String>("key").as_deref(),
        Some("m1")
    );
    assert_eq!(
        delivered.outputs.get::<i32>("a"),
        Some(1),
        "the computed params are the notice's outputs"
    );

    fixture.wait_completed().await;
    assert_eq!(fixture.state("s1").await, TaskState::Completed);
    fixture.close().await;
}

/// A `Msg` step that declares `exposes` exports exactly those names: a
/// name-only entry reads the computed param of that name, an entry with its
/// own `${{ }}` value evaluates against the acting execution's scope, and a
/// name the params do not hold completes as `null`.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_msg_outputs_follow_the_exposes() {
    let mut workflow = Workflow::new()
        .with_id("pack-msg-exposes")
        .with_var("who", "eve");
    workflow.steps.push(crate::Step {
        id: "s1".to_string(),
        name: "s1".to_string(),
        uses: Some(USES_MSG.to_string()),
        params: Vars::new()
            .with("text", "done by ${{ who }}")
            .with("count", 2)
            .to_value(),
        exposes: vec![
            crate::Variant::create("text", "${{ who }}"),
            crate::Variant::new().name("count"),
            crate::Variant::new().name("missing"),
        ],
        ..Default::default()
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let delivered = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(delivered.r#type, "notice");
    assert_eq!(
        delivered.outputs.len(),
        3,
        "the exposes size the outputs bag"
    );
    // The params' own expressions resolved at start: the computed value is
    // the message data in `inputs` and the exported `outputs` alike.
    assert_eq!(
        delivered
            .params()
            .and_then(|params| params.get::<String>("text"))
            .as_deref(),
        Some("done by eve"),
        "the params' expressions evaluate against the act's scope"
    );
    assert_eq!(
        delivered.outputs.get::<String>("text").as_deref(),
        Some("eve"),
        "an exposes entry with its own expression evaluates against the scope, not the param"
    );
    assert_eq!(
        delivered.outputs.get::<i32>("count"),
        Some(2),
        "a name-only exposes entry reads the computed param"
    );
    assert_eq!(
        delivered.outputs.get_value("missing"),
        Some(&serde_json::json!(null)),
        "a name the params do not hold completes as null"
    );

    fixture.wait_completed().await;
    fixture.close().await;
}
