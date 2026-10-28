//! `acts.core.owner` cells over the public engine API and the shared
//! [`FlowFixture`](acts::testing::FlowFixture): the plugin is the only thing
//! installed, and its `on_init` registers the package.

use super::*;

use acts::testing::FlowFixture;
use acts::{Config, Engine, Principal, Step, UserPolicy, Vars, Workflow};
use serde_json::{Value as JsonValue, json};
use std::sync::atomic::{AtomicU64, Ordering};

const USES_OWNER: &str = "acts.core.owner";
const USES_PARALLEL: &str = "acts.core.parallel";
const USES_IRQ: &str = "acts.core.irq";

/// Build the engine through the public API: adding the plugin is enough —
/// its `on_init` registers the `acts.core.owner` package.
async fn deploy(workflow: &Workflow) -> FlowFixture {
    let config = test_config();
    let engine = Engine::builder()
        .set_config(&config)
        .disable_acl()
        .add_plugin(&ParticipantPlugin::new())
        .start()
        .await
        .unwrap();
    FlowFixture::deploy_on(engine, workflow).await
}

/// A config loaded from TOML, the way acts-server builds one.
fn test_config() -> Config {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "acts-participant-{}-{}.toml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut table = toml::Table::new();
    table.insert("cache_cap".to_string(), toml::Value::Integer(64));
    std::fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    let config = Config::create(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    config
}

/// The engine's schema validation and the package's parser accept every
/// documented owner expression, and the request reaches the client with the
/// business atoms intact (the wrapper `{ owner: … }` is stripped).
#[tokio::test(flavor = "multi_thread")]
async fn pack_owner_accepts_the_documented_owner_expressions() {
    let expressions = [
        (
            "owners_of",
            json!({
                "kind": "owners_of",
                "resource": { "resource": "department", "name": "研发部" },
                "duty": "principal"
            }),
            "owners_of",
        ),
        (
            "relate",
            json!({
                "kind": "relate",
                "from": { "kind": "context", "var": { "type": "current_actor" } },
                "rel": { "rel": "resource", "value": "department_of" },
                "duty": "principal"
            }),
            "relate",
        ),
        (
            "any_of",
            json!({
                "kind": "any_of",
                "operands": [
                    { "kind": "context", "var": { "type": "initiator" } },
                    { "kind": "role", "name": "reviewer" }
                ]
            }),
            "any_of",
        ),
        (
            "wrapped",
            json!({ "owner": { "kind": "role", "name": "reviewer" } }),
            "role",
        ),
    ];
    let mut workflow = Workflow::new().with_id("pack-owner-forms");
    for (id, params, _) in &expressions {
        workflow.steps.push(Step {
            id: id.to_string(),
            name: id.to_string(),
            uses: Some(USES_OWNER.to_string()),
            params: params.clone(),
            ..Default::default()
        });
    }

    let mut fixture = deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    for (id, _, expected_kind) in &expressions {
        let created = fixture.wait_created(id).await;
        let params = created
            .params()
            .unwrap_or_else(|| panic!("{id} must send an owner expression"));
        assert_eq!(
            params.get::<String>("kind").as_deref(),
            Some(*expected_kind),
            "{id} sent {params}"
        );
        if *id == "wrapped" {
            assert!(
                params.get_value("owner").is_none(),
                "prepare_params must strip the wrapper: {params}"
            );
        }
        fixture
            .complete(
                &created.pid,
                &created.tid,
                Vars::new().with("ids", json!(["u1"])).with("count", 1),
            )
            .await;
    }
}

/// The engine fills the run-owned context atoms before the request reaches
/// the business system: the client sees a `user`, not an `initiator` it would
/// have to resolve itself.
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
    let mut fixture = deploy(&workflow).await;
    let starter = Principal::from_policy(&UserPolicy {
        name: "starter".to_string(),
        enabled: true,
        allow: vec!["model:deploy".to_string(), "proc:start".to_string()],
        patterns: vec!["pack-owner-context".to_string()],
        ..Default::default()
    })
    .unwrap();
    fixture.start_as(&starter, Vars::new()).await.unwrap();

    let owners = fixture.wait_created("owners").await;
    let params = owners.params().expect("the prepared owner expression");
    assert_eq!(params.get::<String>("kind").as_deref(), Some("user"));
    assert_eq!(params.get::<String>("name").as_deref(), Some("starter"));

    fixture
        .complete(
            &owners.pid,
            &owners.tid,
            Vars::new().with("ids", json!(["u1"])),
        )
        .await;

    let approve = fixture.wait_created("approve").await;
    assert_eq!(approve.owner.as_deref(), Some("u1"));
}

/// The package is an IRQ act: the Created message carries the owner
/// expression, and the client's answer (`participants`/`ids`) is what the
/// next `parallel` fans out. The engine never resolves the business-data
/// atoms itself.
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
                        Step::new()
                            .with_id("approve_act")
                            .with_uses(USES_IRQ, Vars::new().with("value", "${{ value }}")),
                    ],
                ),
            )
        });
    let mut fixture = deploy(&workflow).await;
    fixture.start(Vars::new()).await;

    let owners = fixture.wait_created("owners").await;
    assert_eq!(owners.uses.as_deref(), Some(USES_OWNER));
    let params = owners
        .params()
        .expect("the owner expression rides the request");
    assert_eq!(params.get::<String>("kind").as_deref(), Some("owners_of"));
    assert_eq!(
        params.get::<JsonValue>("resource").unwrap()["name"],
        json!("研发部")
    );

    // The business system answers with its own directory's result.
    fixture
        .complete(
            &owners.pid,
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
        .await;

    let mut values: Vec<String> = fixture
        .wait_created_uses(USES_IRQ, 2)
        .await
        .into_iter()
        .filter_map(|message| {
            message
                .params()
                .and_then(|params| params.get::<String>("value"))
        })
        .collect();
    values.sort();
    assert_eq!(values, ["u1", "u2"]);
}
