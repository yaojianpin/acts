//! End-to-end fetch: the variable registers through `pack:publish` metadata,
//! a workflow expression reads `$profile`, the engine's fetch request reaches
//! only the provider subject that owns the scope (the req gate), and the
//! provider's `snap:reply` seals the value the act finally reads.

use acts::{ChannelOptions, Engine, Principal, UserSpec, Vars, Workflow, actions::apply_as};
use acts_acl::AclUsers;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const TARGET: &str = "profile";

/// The engine with the store-backed user registry, the admin past its
/// initial-state password, and a provider account that owns the target's
/// `t1/*` scopes only.
async fn engine_with_provider() -> (Engine, Principal, Principal) {
    let engine = Engine::builder()
        .with_user_acl()
        .set_store(Arc::new(acts::MemoryStore::new()))
        .start()
        .await
        .unwrap();

    // the builtin admin answers its default password and must change it
    // before any other action goes through
    engine
        .acl()
        .set_user(&UserSpec {
            name: "admin".into(),
            add_passwords: vec!["root-pw".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let tokens = engine.acl().login("admin", "root-pw").await.unwrap();
    let admin = engine.acl().authenticate(Some(&tokens.token)).unwrap();

    // the provider: `@write` (a snap:reply is a data-plane write) and the
    // target's t1 scopes only
    engine
        .acl()
        .set_user(&UserSpec {
            name: "provider".into(),
            add_passwords: vec!["provider-pw".into()],
            allow: Some(vec!["@write".into()]),
            snapshot: Some(HashMap::from([(TARGET.into(), vec!["t1/*".into()])])),
            ..Default::default()
        })
        .await
        .unwrap();
    let tokens = engine.acl().login("provider", "provider-pw").await.unwrap();
    let provider = engine.acl().authenticate(Some(&tokens.token)).unwrap();

    (engine, admin, provider)
}

fn snapshot_patterns(target: &str, pattern: &str) -> Option<HashMap<String, Vec<String>>> {
    Some(HashMap::from([(target.into(), vec![pattern.into()])]))
}

fn context_pairs(pairs: [(&str, &str); 2]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The provider channel: answers every `req` for the target it receives, and
/// counts the replies it managed to land (and the ones its grants refuse).
fn serve_provider(engine: &Engine, provider: &Principal) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let replied = Arc::new(AtomicUsize::new(0));
    let denied = Arc::new(AtomicUsize::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        id: "provider".into(),
        ack: false,
        r#type: "req".into(),
        uses: TARGET.into(),
        ..Default::default()
    });
    let engine = engine.clone();
    let provider = provider.clone();
    let replied_in = replied.clone();
    let denied_in = denied.clone();
    chan.on_message(move |e| {
        let engine = engine.clone();
        let provider = provider.clone();
        let replied = replied_in.clone();
        let denied = denied_in.clone();
        async move {
            let msg = e.inner();
            if msg.r#type != "req" {
                return;
            }
            let req_id = msg.inputs.get::<String>("req_id").unwrap_or_default();
            let scope = msg.inputs.get::<String>("scope").unwrap_or_default();
            let payload = Vars::new()
                .with("req_id", req_id)
                .with("name", TARGET)
                .with("scope", scope.clone())
                .with("rev", 1u64)
                .with("data", Vars::new().with("uid", "who"));
            match apply_as(&engine, &provider, "snap:reply", payload).await {
                Ok(_) => {
                    replied.fetch_add(1, Ordering::SeqCst);
                }
                Err(err) => {
                    eprintln!("reply refused: {err}");
                    denied.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    (replied, denied)
}

/// The act watcher: records the first `created` step message — its params
/// prove the fetch sealed before the params fill evaluated — plus the
/// pid/tid the completion action needs.
fn watch_act(
    engine: &Engine,
) -> Arc<std::sync::Mutex<Option<(String, String, serde_json::Value)>>> {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let chan = engine.channel_with_options(&ChannelOptions {
        id: "watcher".into(),
        ack: false,
        r#type: "step".into(),
        state: "created".into(),
        ..Default::default()
    });
    let seen_in = seen.clone();
    chan.on_message(move |e| {
        let seen = seen_in.clone();
        async move {
            let msg = e.inner();
            if msg.uses.as_deref() == Some("acts.core.irq")
                && let Some(params) = msg.params()
            {
                let mut slot = seen.lock().unwrap();
                if slot.is_none() {
                    *slot = Some((msg.pid.clone(), msg.tid.clone(), params.to_value()));
                }
            }
        }
    });
    seen
}

fn fetch_workflow(id: &str, rn: &str) -> Workflow {
    Workflow::new().with_id(id).with_rn(rn).with_step(|step| {
        step.with_id("s1").with_uses(
            "acts.core.irq",
            Vars::new().with("p", "${{ $profile.uid }}"),
        )
    })
}

async fn publish_variable(engine: &Engine, admin: &Principal, on_missing: &str, timeout: u64) {
    apply_as(
        engine,
        admin,
        "pack:publish",
        Vars::new()
            .with("id", "acts.clientA.profile")
            .with("run_as", "irq")
            .with("catalog", "app")
            .with(
                "options",
                json!({"var": TARGET, "timeout_ms": timeout, "on_missing": on_missing}),
            ),
    )
    .await
    .unwrap();
}

/// The one registration knob each scenario needs: the run's owner reads only
/// its own tenant's snapshot scopes and may start only its tenant's
/// workflows (`rn` `t1:*`/`t2:*`).
async fn add_runner(
    engine: &Engine,
    name: &str,
    password: &str,
    tenant: &str,
    context: Option<BTreeMap<String, String>>,
) -> Principal {
    engine
        .acl()
        .set_user(&UserSpec {
            name: name.into(),
            add_passwords: vec![password.into()],
            allow: Some(vec!["@execute".into(), "msg:ack".into()]),
            patterns: Some(vec![format!("{tenant}:*")]),
            snapshot: snapshot_patterns(TARGET, &format!("{tenant}/*")),
            context,
            ..Default::default()
        })
        .await
        .unwrap();
    let tokens = engine.acl().login(name, password).await.unwrap();
    engine.acl().authenticate(Some(&tokens.token)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fetch_reaches_only_the_owner_and_the_reply_seals_the_expression() {
    let (engine, admin, provider) = engine_with_provider().await;
    publish_variable(&engine, &admin, "error", 2000).await;

    let (replied, _denied) = serve_provider(&engine, &provider);
    let seen = watch_act(&engine);

    // the registration is the publish: `options.var` names the variable and
    // its pull parameters
    let workflow = fetch_workflow("fetch-e2e", "t1:fetch");
    engine
        .executor(&admin)
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    // the owner's context composes the scope key: `t1/u9`
    let op = add_runner(
        &engine,
        "op",
        "op-pw",
        "t1",
        Some(context_pairs([("tenant", "t1"), ("unit", "u9")])),
    )
    .await;
    let handle = engine
        .executor(&op)
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (pid, tid, params) = loop {
        let seen = seen.lock().unwrap().clone();
        if let Some(seen) = seen {
            break seen;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the act message never arrived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(
        params.get("p"),
        Some(&json!("who")),
        "the expression read the fetched value; params={params:?} replied={} denied={}",
        replied.load(Ordering::SeqCst),
        _denied.load(Ordering::SeqCst)
    );

    // complete through the action table: the run settles completed
    apply_as(
        &engine,
        &admin,
        "act:complete",
        Vars::new().with("pid", &pid).with("tid", &tid),
    )
    .await
    .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.wait())
        .await
        .expect("the run settles")
        .unwrap();
    assert_eq!(outcome, acts::RunOutcome::Completed, "{outcome:?}");
    assert_eq!(replied.load(Ordering::SeqCst), 1);

    // the registration names its provider packages: an operator can tell
    // who serves a same-named variable
    let rows = apply_as(&engine, &admin, "snap:ls", Vars::new().with("name", TARGET))
        .await
        .unwrap();
    assert_eq!(
        rows[0]["providers"],
        json!(["acts.clientA.profile"]),
        "{rows}"
    );
    engine.close().await;
}

/// A run whose scope the provider cannot read: the req gate withholds the
/// request from the provider's channel entirely, the fetch times out, and —
/// with `on_missing = error` — the run fails instead of leaking the name.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_outside_the_provider_scopes_is_withheld_and_the_run_fails() {
    let (engine, admin, provider) = engine_with_provider().await;
    publish_variable(&engine, &admin, "error", 300).await;

    let (replied, denied) = serve_provider(&engine, &provider);
    let _seen = watch_act(&engine);

    let workflow = fetch_workflow("fetch-denied", "t2:fetch");
    engine
        .executor(&admin)
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    // the run's owner reads t2 — the provider does not
    let op2 = add_runner(&engine, "op2", "op2-pw", "t2", None).await;
    let handle = engine
        .executor(&op2)
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outcome = tokio::time::timeout(Duration::from_secs(15), handle.wait())
        .await
        .expect("the run settles")
        .unwrap();
    assert!(
        matches!(outcome, acts::RunOutcome::Errored { .. }),
        "the withheld fetch must fail the seal, not leak: {outcome:?}"
    );
    assert_eq!(
        replied.load(Ordering::SeqCst),
        0,
        "the provider must never land a reply for a scope it cannot read"
    );
    assert_eq!(
        denied.load(Ordering::SeqCst),
        0,
        "the provider must never even be asked"
    );
    engine.close().await;
}
