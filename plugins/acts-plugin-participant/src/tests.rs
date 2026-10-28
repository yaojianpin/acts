//! `acts.core.owner` cells over the public engine API and the shared
//! [`FlowFixture`](acts::testing::FlowFixture): the plugin is the only thing
//! installed, and its `on_init` registers the package.

use super::*;

use acts::testing::FlowFixture;
use acts::{
    Config, Context, Engine, MessageState, OwnerExpr, Participant, ParticipantResolver, Principal,
    Step, UserPolicy, Vars, Workflow,
};
use serde_json::{Value as JsonValue, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const USES_OWNER: &str = "acts.core.owner";
const USES_PARALLEL: &str = "acts.core.parallel";
const USES_IRQ: &str = "acts.core.irq";

/// Build the engine through the public API: adding the plugin is enough —
/// its `on_init` registers the `acts.core.owner` package and the resolver.
async fn start_engine(plugin: ParticipantPlugin) -> Engine {
    let config = test_config();
    Engine::builder()
        .set_config(&config)
        .disable_acl()
        .add_plugin(&plugin)
        .start()
        .await
        .unwrap()
}

async fn deploy_with(workflow: &Workflow, plugin: ParticipantPlugin) -> FlowFixture {
    FlowFixture::deploy_on(start_engine(plugin).await, workflow).await
}

async fn deploy(workflow: &Workflow) -> FlowFixture {
    deploy_with(workflow, ParticipantPlugin::new()).await
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

/// A custom resolver: a naming rule maps the local user `alice` to `bob`;
/// every other expression delegates to the plugin's default. It proves the
/// port is consulted on the assign start path and that a replacement can
/// reuse the local evaluation instead of copying it.
struct ReplaceResolver;

impl ParticipantResolver for ReplaceResolver {
    fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> acts::Result<Vec<Participant>> {
        if let OwnerExpr::User { name } = expr
            && name == "alice"
        {
            return Ok(vec![Participant::new("bob")]);
        }
        LocalParticipantResolver.resolve(ctx, expr)
    }
}

/// The same naming rule, but it resolves `alice` to two participants: the
/// kernel's single-owner rule must still refuse the act.
struct MultiResolver;

impl ParticipantResolver for MultiResolver {
    fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> acts::Result<Vec<Participant>> {
        if let OwnerExpr::User { name } = expr
            && name == "alice"
        {
            return Ok(vec![Participant::new("bob"), Participant::new("carol")]);
        }
        LocalParticipantResolver.resolve(ctx, expr)
    }
}

fn assigned_user_workflow(id: &str, owner: &str) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(
        Step::new()
            .with_id("approve")
            .with_name("approve")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_assign_expr(OwnerExpr::User {
                name: owner.to_string(),
            }),
    );
    workflow
}

fn approved_user(name: &str) -> Principal {
    Principal::from_policy(&UserPolicy {
        name: name.to_string(),
        enabled: true,
        allow: vec!["act:complete".to_string()],
        ..Default::default()
    })
    .unwrap()
}

/// `ParticipantPlugin::with_resolver` installs a replacement resolver: the
/// `Created` message carries the owner the port resolved, and an expression
/// the replacement does not handle still uses the local evaluation.
#[tokio::test(flavor = "multi_thread")]
async fn participant_resolver_replaces_the_assign_owner() {
    let workflow = Workflow::new()
        .with_id("participant-resolver")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_assign_expr(OwnerExpr::User {
                    name: "alice".to_string(),
                })
        })
        .with_step(|step| {
            step.with_id("review")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
                .with_assign_expr(OwnerExpr::User {
                    name: "carol".to_string(),
                })
        });
    let mut fixture = deploy_with(
        &workflow,
        ParticipantPlugin::with_resolver(Arc::new(ReplaceResolver)),
    )
    .await;
    fixture.start(Vars::new()).await;

    let approve = fixture.wait_created("approve").await;
    assert_eq!(
        approve.owner.as_deref(),
        Some("bob"),
        "assign.owner must go through the installed resolver"
    );

    fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(fixture.pid(), &approve.tid, Vars::new())
        .await
        .unwrap();

    let review = fixture.wait_created("review").await;
    assert_eq!(
        review.owner.as_deref(),
        Some("carol"),
        "an expression the replacement does not handle delegates to LocalParticipantResolver"
    );
    fixture.close().await;
}

/// Replacing the resolver changes only who is resolved. The kernel still
/// authorizes the decision from `__assignment`: the old target (`alice`) is
/// refused, the resolved owner (`bob`) decides.
#[tokio::test(flavor = "multi_thread")]
async fn participant_resolver_does_not_change_decision_authorization() {
    let workflow = assigned_user_workflow("participant-auth", "alice");
    let mut fixture = deploy_with(
        &workflow,
        ParticipantPlugin::with_resolver(Arc::new(ReplaceResolver)),
    )
    .await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("approve").await;
    assert_eq!(created.owner.as_deref(), Some("bob"));

    let refused = fixture
        .engine()
        .executor(&approved_user("alice"))
        .act()
        .complete(fixture.pid(), &created.tid, Vars::new())
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("ENOTOWNER"),
        "the expression's user must not decide after the resolver moved the owner: {refused}"
    );

    fixture
        .engine()
        .executor(&approved_user("bob"))
        .act()
        .complete(fixture.pid(), &created.tid, Vars::new())
        .await
        .unwrap();
    fixture
        .wait_message("approve", MessageState::Completed)
        .await;
    fixture.close().await;
}

/// The resolver slot is sealed by the start pass: a replacement registered
/// after `EngineBuilder::start` returns is refused and the start-time
/// resolver stays in effect.
#[tokio::test(flavor = "multi_thread")]
async fn participant_resolver_is_sealed_after_start() {
    let engine = start_engine(ParticipantPlugin::with_resolver(Arc::new(ReplaceResolver))).await;
    let sealed = engine
        .register_capability_provider(acts::capability::CapabilityProvider {
            kind: acts::capability::CapabilityKind::Participant,
            key: "late.capability".to_string(),
            plugin: "late-plugin".to_string(),
            version: "0".to_string(),
        })
        .unwrap_err();
    assert!(
        sealed.to_string().contains("sealed"),
        "a post-start capability advertisement must be refused: {sealed}"
    );
    let error = engine
        .register_participant_resolver(crate::PROVIDER, Arc::new(LocalParticipantResolver))
        .unwrap_err();
    assert!(
        matches!(error, acts::ActError::Runtime(_)),
        "a post-start replacement must be a runtime refusal: {error:?}"
    );
    assert!(
        error.to_string().contains("ActPlugin::on_init"),
        "the refusal must name the startup hook: {error}"
    );

    let workflow = assigned_user_workflow("participant-sealed", "alice");
    let mut fixture = FlowFixture::deploy_on(engine, &workflow).await;
    fixture.start(Vars::new()).await;
    let created = fixture.wait_created("approve").await;
    assert_eq!(
        created.owner.as_deref(),
        Some("bob"),
        "the failed replacement must leave the start-time resolver in place"
    );
    fixture.close().await;
}

/// A resolver that answers several participants does not change `assign`'s
/// single-owner rule: the act fails and names the resolution.
#[tokio::test(flavor = "multi_thread")]
async fn participant_resolver_multi_result_still_requires_one_owner() {
    let workflow = assigned_user_workflow("participant-multi", "alice");
    let mut fixture = deploy_with(
        &workflow,
        ParticipantPlugin::with_resolver(Arc::new(MultiResolver)),
    )
    .await;
    fixture.start(Vars::new()).await;

    let failed = fixture.wait_message("approve", MessageState::Error).await;
    let message = failed.outputs.get::<String>("message").unwrap_or_default();
    assert!(
        message.contains("exactly one owner")
            && message.contains("bob")
            && message.contains("carol"),
        "the failure must name the multi-owner resolution: {message}"
    );
    fixture.wait_error().await;
    fixture.close().await;
}

/// A second participant provider, used to prove explicit provider selection.
/// It maps the local user `alice` to `bob`, so the resolved owner names which
/// resolver ran.
#[derive(Clone)]
struct SecondParticipantPlugin;

const SECOND_PROVIDER: &str = "test.second";

#[async_trait::async_trait]
impl acts::ActPlugin for SecondParticipantPlugin {
    fn on_init(&self, engine: &Engine) -> acts::Result<()> {
        engine.register_capability_provider(acts::capability::CapabilityProvider {
            kind: acts::capability::CapabilityKind::Participant,
            key: SECOND_PROVIDER.to_string(),
            plugin: SECOND_PROVIDER.to_string(),
            version: "0".to_string(),
        })?;
        engine.register_participant_resolver(SECOND_PROVIDER, Arc::new(ReplaceResolver))
    }
}

async fn start_engine_with_two_providers() -> Engine {
    let config = test_config();
    Engine::builder()
        .set_config(&config)
        .disable_acl()
        .add_plugin(&ParticipantPlugin::new())
        .add_plugin(&SecondParticipantPlugin)
        .start()
        .await
        .unwrap()
}

/// An explicit `assign.provider` selects that provider's resolver, and the
/// unkeyed provider is not consulted.
#[tokio::test(flavor = "multi_thread")]
async fn explicit_assignment_provider_selects_its_resolver() {
    let workflow = Workflow::new()
        .with_id("participant-explicit-provider")
        .with_step(|step| {
            step.with_id("approve")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                .with_assign_expr(OwnerExpr::User {
                    name: "alice".to_string(),
                })
                .with_assign_provider(SECOND_PROVIDER)
        });
    let engine = start_engine_with_two_providers().await;
    let mut fixture = FlowFixture::deploy_on(engine, &workflow).await;
    fixture.start(Vars::new()).await;

    let created = fixture.wait_created("approve").await;
    assert_eq!(
        created.owner.as_deref(),
        Some("bob"),
        "the declared provider's resolver must resolve the owner"
    );
    fixture.close().await;
}

/// With two providers and no `assign.provider`, publish must fail instead of
/// choosing one silently.
#[tokio::test(flavor = "multi_thread")]
async fn unkeyed_assignment_with_multiple_providers_is_refused() {
    let engine = start_engine_with_two_providers().await;
    let workflow = assigned_user_workflow("participant-unkeyed-multi", "alice");
    let error = engine
        .executor(&Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("multiple providers"),
        "publish must refuse an ambiguous provider: {error}"
    );
    assert!(
        error.to_string().contains("assign.owner"),
        "the refusal must name the model attribute: {error}"
    );
    engine.close().await;
}

/// An explicit provider key that no plugin registered is refused by name.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_assignment_provider_is_refused() {
    let engine = start_engine(ParticipantPlugin::new()).await;
    let workflow = assigned_user_workflow("participant-unknown-provider", "alice");
    let mut workflow = workflow;
    workflow.steps[0].assign.as_mut().unwrap().provider = Some("missing.provider".to_string());
    let error = engine
        .executor(&Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("missing.provider"),
        "the refusal must name the missing provider: {error}"
    );
    engine.close().await;
}
