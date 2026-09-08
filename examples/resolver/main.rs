use acts::{Engine, Result, SnapshotOptions, Vars, Workflow};

/// Snapshot-backed sealed data demo.
///
/// External systems push versioned configuration into the engine through
/// `engine.snapshot()` (a gRPC/NATS/Kafka feed adapter does the same over
/// the wire). At each task's prepare the engine seals the *local* snapshot
/// value for the target's scope — `resolve` never performs network I/O.
///
/// `profile` uses `PerProc`: the value is frozen when a process starts and
/// inherited by every descendant task. Feeding a new revision mid-run only
/// affects processes started afterwards.
#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::builder()
        .add_snapshot("profile", SnapshotOptions::per_proc())
        .build()
        .start()
        .await?;

    // --- feed v1 (simulates data arriving from another system) ---------------
    engine.snapshot().upsert(
        "profile",
        "",
        1,
        Vars::new()
            .with("tenant", "acme-corp")
            .with(
                "secrets",
                Vars::new()
                    .with("API_KEY", "sk-abc123")
                    .with("DB_PASS", "s3cr3t"),
            )
            .with(
                "features",
                Vars::new().with("beta", true).with("rate_limit", 100),
            ),
    );

    let workflow = Workflow::new()
        .with_id("snapshot_demo")
        .with_ver("0.1.0")
        .with_step(|step| {
            step.with_id("step1")
                .with_name("access sealed config")
                .with_uses_code(
                    "acts.transform.code",
                    r#"
                // Access sealed data injected from the snapshot
                let tenant = $profile.tenant;
                let apiKey = $profile.secrets.API_KEY;
                let beta = $profile.features.beta;
                let rateLimit = $profile.features.rate_limit;

                console.log("tenant:", tenant);
                console.log("apiKey:", apiKey);
                console.log("beta:", beta);

                $set("output", "tenant=" + tenant + ", beta=" + beta + ", rateLimit=" + rateLimit);
                "#,
                )
        });

    workflow.print();

    let executor = engine.executor();
    executor.model().deploy(&workflow, None).await?;

    // --- run 1: seals v1 ---------------------------------------------
    let (s1, sig1) = engine.signal(()).double();
    let executor1 = executor.clone();
    executor1
        .proc()
        .start(&workflow.id, Vars::new().with("pid", "r1"))
        .await?;

    engine.channel().on_complete(move |e| {
        let s1 = s1.clone();
        async move {
            if e.pid != "r1" {
                return;
            }
            println!("run1 on_complete: {:?}, cost={}ms", e.outputs, e.cost());
            s1.close();
        }
    });
    engine.channel().on_error(move |e| async move {
        if e.pid == "r1" {
            println!("run1 on_error: {:?}", e.state);
        }
    });
    sig1.recv().await;

    // --- external system publishes v2 ----------------------------------------
    engine.snapshot().upsert(
        "profile",
        "",
        2,
        Vars::new()
            .with("tenant", "acme-corp")
            .with(
                "secrets",
                Vars::new()
                    .with("API_KEY", "sk-abc123")
                    .with("DB_PASS", "s3cr3t"),
            )
            .with(
                "features",
                Vars::new().with("beta", true).with("rate_limit", 200),
            ),
    );
    let current = engine.snapshot().read("profile", "").unwrap();
    println!(
        "snapshot current: rev={}, rate_limit={}",
        current.rev,
        current
            .data
            .get::<Vars>("features")
            .unwrap()
            .get::<i32>("rate_limit")
            .unwrap(),
    );

    // --- run 2: a new process seals v2 ---------------------------------------
    let (s2, sig2) = engine.signal(()).double();
    executor1
        .proc()
        .start(&workflow.id, Vars::new().with("pid", "r2"))
        .await?;

    engine.channel().on_complete(move |e| {
        let s2 = s2.clone();
        async move {
            if e.pid != "r2" {
                return;
            }
            println!("run2 on_complete: {:?}, cost={}ms", e.outputs, e.cost());
            s2.close();
        }
    });
    engine.channel().on_error(move |e| async move {
        if e.pid == "r2" {
            println!("run2 on_error: {:?}", e.state);
        }
    });
    sig2.recv().await;

    engine.close().await;
    Ok(())
}
