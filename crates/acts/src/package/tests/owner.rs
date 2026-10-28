//! `acts.core.owner` cells: the request the engine sends and the fan-out the
//! business system's answer drives.

use super::*;
use crate::{MessageState, Principal, UserPolicy, Vars};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

/// The package is an IRQ act: the Created message carries the owner
/// expression, and the client's answer (`participants`/`ids`) is what the
/// next `parallel` fans out. The engine never resolves the business-data
/// atoms itself.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_owner_sends_the_expression_and_fans_out_the_answer() {
    let workflow = Workflow::new()
        .with_id("pack-owner")
        .with_step(|step| {
            step.with_id("owners").with_uses(
                USES_OWNER,
                Vars::new()
                    .with("kind", "owners_of")
                    .with(
                        "resource",
                        json!({ "resource": "department", "name": "研发部" }),
                    )
                    .with("duty", "principal"),
            )
        })
        .with_step(|step| {
            step.with_id("approve").with_uses(
                USES_PARALLEL,
                Vars::new().with("in", "${{ owners.ids }}").with(
                    "steps",
                    vec![
                        crate::model::Step::new()
                            .with_id("approve_act")
                            .with_uses(USES_IRQ, Vars::new().with("value", "${{ value }}")),
                    ],
                ),
            )
        });
    let mut fixture = FlowFixture::deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let owners = fixture.wait_created("owners").await;
    assert_eq!(owners.uses.as_deref(), Some(USES_OWNER));
    assert_eq!(owners.state, MessageState::Created);
    let params = owners
        .inputs
        .get::<Vars>("params")
        .expect("the owner expression rides the request");
    assert_eq!(params.get::<String>("kind").as_deref(), Some("owners_of"));
    assert_eq!(
        params.get::<serde_json::Value>("resource").unwrap()["name"],
        json!("研发部")
    );

    // The business system answers with its own directory's result.
    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .act()
        .complete(
            fixture.pid(),
            &owners.tid,
            Vars::new()
                .with(
                    "participants",
                    json!([
                        { "id": "u1", "code": "alice", "name": "Alice" },
                        { "id": "u2", "code": "bob", "name": "Bob" }
                    ]),
                )
                .with("ids", json!(["u1", "u2"]))
                .with("count", 2),
        )
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let created = fixture
            .messages()
            .iter()
            .filter(|message| {
                message.state == MessageState::Created && message.uses.as_deref() == Some(USES_IRQ)
            })
            .count();
        if created >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the two fan-out acts; saw {created}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mut values: Vec<String> = fixture
        .messages()
        .into_iter()
        .filter(|message| {
            message.state == MessageState::Created && message.uses.as_deref() == Some(USES_IRQ)
        })
        .filter_map(|message| {
            message
                .params()
                .and_then(|params| params.get::<String>("value"))
        })
        .collect();
    values.sort();
    assert_eq!(values, ["u1", "u2"]);

    fixture.close().await;
}

/// The engine fills the run-owned context atoms before the request reaches
/// the business system: the client sees a `user`, not an `initiator` it would
/// have to resolve itself.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_owner_prepares_context_atoms_for_the_business_system() {
    let workflow = Workflow::new()
        .with_id("pack-owner-context")
        .with_rn("pack-owner-context")
        .with_step(|step| {
            step.with_id("owners").with_uses(
                USES_OWNER,
                Vars::new()
                    .with("kind", "context")
                    .with("var", json!({ "type": "initiator" })),
            )
        })
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(USES_IRQ, Vars::new())
                .with_assign("${{ owners.ids[0] }}")
        });
    let fixture = FlowFixture::deploy(&workflow).await;
    let starter = Principal::from_policy(&UserPolicy {
        name: "starter".to_string(),
        enabled: true,
        allow: vec!["model:deploy".to_string(), "proc:start".to_string()],
        patterns: vec!["pack-owner-context".to_string()],
        ..Default::default()
    })
    .unwrap();
    fixture
        .engine()
        .executor(&starter)
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let owners = fixture.wait_created("owners").await;
    let params = owners
        .inputs
        .get::<Vars>("params")
        .expect("the prepared owner expression");
    assert_eq!(params.get::<String>("kind").as_deref(), Some("user"));
    assert_eq!(params.get::<String>("name").as_deref(), Some("starter"));

    fixture
        .engine()
        .executor(&Principal::unrestricted())
        .act()
        .complete(
            &owners.pid,
            &owners.tid,
            Vars::new().with("ids", json!(["u1"])),
        )
        .await
        .unwrap();

    let approve = fixture.wait_created("approve").await;
    assert_eq!(approve.owner.as_deref(), Some("u1"));

    fixture.close().await;
}
