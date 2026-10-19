//! Message cells migrated from the pre-kernel scheduler suite
//! (`tests/message.rs`): the engine's `Message` shape for the workflow root,
//! a package-less step and an act, the times it carries, and the channel
//! filters that select it.
//!
//! Two mappings are deliberate. The legacy cells read the workflow model from
//! `inputs.model`; the kernel keeps that bag on every task/act payload, while
//! `mid` rides the act payload only. And a step's legacy "outputs" assertion
//! is now the message's `inputs`/`outputs` split: a package-less step
//! self-completes without scope writes, so a client-decided (blank `uses`)
//! step is what produces completion outputs here.

use std::sync::Arc;

use parking_lot::Mutex;
use serial_test::serial;

use crate::{
    ChannelOptions, Message, MessageState, Vars, Workflow,
    event::EventAction,
    scheduler::core::model::task_graph::TaskState,
    utils::{
        consts,
        test::{USES_IRQ, USES_SET},
    },
};

use super::{FlowFixture, Terminal};

fn completed(terminal: &Terminal) {
    assert!(
        matches!(terminal, Terminal::Completed(_)),
        "the run must complete, got {terminal:?}"
    );
}

// ---------------------------------------------------------------------------
// Workflow lifecycle
// ---------------------------------------------------------------------------

/// The root's `Created` message is the task-shaped workflow announcement: its
/// type is `workflow`, its node is the root (`$`), the model facts ride
/// `inputs.model`, and the run's variables are its inputs (the legacy
/// `sch_message_workflow_*` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_workflow_created_message_names_the_model_and_its_vars() {
    let workflow = Workflow::new()
        .with_id("flow-msg-workflow")
        .with_name("Orders")
        .with_option("tag", "my_tag")
        .with_var("a", 5)
        .with_step(|step| step.with_id("s1"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created(consts::TASK_ROOT_TID).await;
    assert_eq!(created.r#type, "workflow");
    assert_eq!(created.nid, consts::TASK_ROOT_TID);
    assert_eq!(created.name, "Orders");
    assert_eq!(created.pid, fixture.pid());
    assert!(created.start_time > 0);

    let model = created
        .inputs
        .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
        .expect("the workflow model rides the message");
    assert_eq!(
        model.get::<String>("id").as_deref(),
        Some("flow-msg-workflow")
    );
    assert_eq!(model.get::<String>("name").as_deref(), Some("Orders"));
    assert_eq!(
        model
            .get::<Vars>("options")
            .and_then(|options| options.get::<String>("tag")),
        Some("my_tag".to_string())
    );
    assert_eq!(
        created.inputs.get::<i32>("a"),
        Some(5),
        "the run's variables are the message's inputs"
    );

    fixture.wait_completed().await;
    fixture.close().await;
}

/// A completed run reports its model's `exposes` as the lifecycle outputs
/// (the legacy `sch_message_workflow_outputs`, now read from the completion
/// event the fixture collects).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_workflow_completion_reports_its_exposes() {
    let workflow = Workflow::new()
        .with_id("flow-msg-exposes")
        .with_var("value", 0)
        .with_expose(crate::Variant::create("result", "${{ value }}"))
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_SET, Vars::new().with("value", 42))
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);

    assert_eq!(fixture.outputs().get::<i32>("result"), Some(42));
    fixture.close().await;
}

/// The forwarder stamps the window on every announced record: `Created`
/// carries a start, the terminal record closes it, and `cost()` is the window
/// (the legacy `sch_message_time` cell, extended to the root).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_times_span_the_execution() {
    let workflow = Workflow::new()
        .with_id("flow-msg-time")
        .with_step(|step| step.with_id("s1"));
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    assert!(created.start_time > 0, "a created record opens the window");
    assert_eq!(created.end_time, 0);
    assert_eq!(created.cost(), 0, "an open window has no cost");

    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert!(completed.start_time > 0);
    assert!(
        completed.end_time >= completed.start_time,
        "the terminal record closes the same window"
    );
    assert_eq!(completed.cost(), completed.end_time - completed.start_time);

    let root_created = fixture.wait_created(consts::TASK_ROOT_TID).await;
    let root_completed = fixture
        .wait_message(consts::TASK_ROOT_TID, MessageState::Completed)
        .await;
    assert!(root_created.start_time > 0);
    assert!(root_completed.end_time >= root_completed.start_time);
    assert_eq!(
        root_completed.cost(),
        root_completed.end_time - root_completed.start_time
    );

    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Step and act messages
// ---------------------------------------------------------------------------

/// A node without a package announces itself as `step` on both its `Created`
/// and `Completed` records, with the declared options as the message's
/// options bag. The blank `uses` keeps the node a package-less *step* (the
/// message kind the kernel pins) while leaving the completion to the client,
/// which is what lets a step completion carry outputs.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_step_messages_announce_created_and_completed() {
    let workflow = Workflow::new().with_id("flow-msg-step").with_step(|step| {
        step.with_id("s1")
            .with_name("Review")
            .with_var("a", 5)
            .with_option("tag", "my_step_tag")
            // A declared blank package is not a package: the node stays a
            // `step` message and waits for the client.
            .with_uses("   ", Vars::new())
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    assert_eq!(created.r#type, "step");
    assert_eq!(created.nid, "s1");
    assert_eq!(created.name, "Review");
    assert_eq!(created.uses, None);
    assert_eq!(
        created
            .options()
            .and_then(|options| options.get::<String>("tag")),
        Some("my_step_tag".to_string())
    );
    assert_eq!(created.inputs.get::<i32>("a"), Some(5));
    assert_eq!(
        created
            .inputs
            .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
            .and_then(|model| model.get::<String>("id")),
        Some("flow-msg-step".to_string())
    );
    assert_eq!(fixture.state("s1").await, TaskState::Waiting);

    fixture
        .decide(
            &created.pid,
            &created.tid,
            EventAction::Next,
            Vars::new().with("a", 7),
        )
        .await
        .unwrap();

    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.r#type, "step");
    assert_eq!(completed.nid, "s1");
    assert_eq!(
        completed.outputs.get::<i32>("a"),
        Some(7),
        "the client's completion inputs are the step's outputs"
    );
    assert_eq!(
        fixture.announced("s1"),
        vec![MessageState::Created, MessageState::Completed]
    );

    fixture.wait_completed().await;
    fixture.close().await;
}

/// A `uses` step announces an `act` message: the package, the resolved params,
/// the declared options, the model id and the step link on its inputs, then
/// the completion outputs the client supplied (the legacy
/// `sch_message_act_created`/`_tag`/`_outputs`/`_completed` cells).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_act_messages_carry_the_act_payload() {
    let workflow = Workflow::new().with_id("flow-msg-act").with_step(|step| {
        step.with_id("s1")
            .with_name("Approve")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1").with("amount", 10))
            .with_option("tag", "my_act_tag")
    });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("s1").await;
    assert_eq!(created.r#type, "act");
    assert_eq!(created.nid, "s1");
    assert!(!created.tid.is_empty(), "an execution instance id");
    assert_eq!(created.name, "Approve");
    assert_eq!(created.uses.as_deref(), Some(USES_IRQ));
    assert!(created.is_irq());
    assert_eq!(created.mid, "flow-msg-act");
    assert_eq!(
        created
            .params()
            .and_then(|params| params.get::<String>("key")),
        Some("act1".to_string())
    );
    assert_eq!(
        created
            .params()
            .and_then(|params| params.get::<i32>("amount")),
        Some(10)
    );
    assert_eq!(
        created
            .options()
            .and_then(|options| options.get::<String>("tag")),
        Some("my_act_tag".to_string())
    );

    let step = created
        .inputs
        .get::<serde_json::Value>(consts::STEP_KEY)
        .expect("an act message carries its step link");
    assert_eq!(step[consts::STEP_NODE_ID], serde_json::json!("s1"));
    assert_eq!(step[consts::STEP_NODE_NAME], serde_json::json!("Approve"));
    assert_eq!(
        step[consts::STEP_TASK_ID],
        serde_json::json!(created.tid.clone())
    );

    fixture
        .decide(
            &created.pid,
            &created.tid,
            EventAction::Next,
            Vars::new().with("approved", true),
        )
        .await
        .unwrap();

    let completed = fixture.wait_message("s1", MessageState::Completed).await;
    assert_eq!(completed.r#type, "act");
    assert!(completed.is_irq());
    assert_eq!(completed.mid, "flow-msg-act");
    assert_eq!(
        completed.outputs.get::<bool>("approved"),
        Some(true),
        "the act's completion outputs ride the terminal message"
    );
    assert_eq!(
        completed
            .options()
            .and_then(|options| { options.get::<String>("tag") }),
        Some("my_act_tag".to_string())
    );

    fixture.wait_completed().await;
    fixture.close().await;
}

/// A branch announces nothing: its own state changes never reach the channel
/// (the legacy `sch_message_branch_no_message`).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_branch_announces_no_message() {
    let workflow = Workflow::new()
        .with_id("flow-msg-branch")
        .with_step(|step| {
            step.with_id("s1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if("false")
                    .with_step(|inner| inner.with_id("i1"))
            })
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    completed(&fixture.run(Vars::new()).await);

    assert!(
        fixture.messages_of("b1").is_empty(),
        "the branch announced: {:?}",
        fixture.messages_of("b1")
    );
    assert!(
        fixture
            .messages()
            .iter()
            .all(|message| message.r#type != "branch")
    );
    fixture.close().await;
}

// ---------------------------------------------------------------------------
// Channel filters
// ---------------------------------------------------------------------------

/// A channel subscribed with an `options` filter receives only the messages
/// whose options match (the legacy `sch_message_channel_options_match`): the
/// tagged step's `Created`/`Completed`, never the untagged step or the
/// workflow root.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_message_channel_filters_messages_by_their_options() {
    let workflow = Workflow::new()
        .with_id("flow-msg-channel")
        .with_step(|step| step.with_id("plain").with_name("Plain"))
        .with_step(|step| {
            step.with_id("tagged")
                .with_name("Tagged")
                .with_option("tag", "important")
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;

    let received: Arc<Mutex<Vec<Message>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    fixture
        .engine()
        .channel_with_options(&ChannelOptions {
            id: "flow-msg-channel-filter".to_string(),
            options: Vars::new().with("tag", "important"),
            ..Default::default()
        })
        .on_message(move |event| {
            let sink = sink.clone();
            async move {
                sink.lock().push(event.inner().clone());
            }
        });

    completed(&fixture.run(Vars::new()).await);

    // The tagged step is the run's last matching producer; once its completed
    // record reached the filtered channel, every matching message has been
    // offered to it. The unfiltered channel must have seen the untagged step's
    // own record, so the filter has something to reject.
    crate::scheduler::tests::reliability::poll_until(
        "the unfiltered channel to receive the untagged step",
        || async {
            fixture
                .messages()
                .iter()
                .any(|message| message.nid == "plain")
        },
    )
    .await;
    crate::scheduler::tests::reliability::poll_until(
        "the filtered channel to receive the tagged step's completion",
        || async {
            received
                .lock()
                .iter()
                .any(|message| message.nid == "tagged" && message.state == MessageState::Completed)
        },
    )
    .await;

    let received = received.lock().clone();
    assert!(!received.is_empty(), "the filter matched nothing");
    for message in &received {
        assert_eq!(
            message
                .options()
                .and_then(|options| options.get::<String>("tag")),
            Some("important".to_string()),
            "only messages whose options match may arrive: {message:?}"
        );
        assert_eq!(message.nid, "tagged");
    }
    assert!(
        received
            .iter()
            .any(|message| message.state == MessageState::Created)
    );
    assert!(
        received
            .iter()
            .any(|message| message.state == MessageState::Completed)
    );
    assert!(
        fixture
            .messages()
            .iter()
            .any(|message| message.nid == "plain"),
        "the unfiltered channel saw the untagged step"
    );

    fixture.close().await;
}
