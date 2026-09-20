//! The database lease end to end: a live engine commits its writes through the
//! lease fence, and an instance that loses the lease has its writes refused and
//! its engine stopped — the failure the lease exists to prevent, exercised
//! through the same `open_store`/`engine_builder`/`keep_lease` path
//! `acts-server` runs.

use acts::{ActError, KvStore, Principal, Vars, Workflow};
use acts_server::{DbConfig, DbType, ServerPlugins, engine_builder, open_store};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Scratch directory for one case, wiped so it starts from an empty database.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("acts-lease-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The server's `[db]` for an sqlite database, with a lease short enough for a
/// test to watch renewals without waiting out a production TTL.
fn db_config(dir: &Path, owner: &str) -> DbConfig {
    DbConfig {
        kind: DbType::Sqlite,
        database_url: Some(dir.join("acts.db").to_string_lossy().into_owned()),
        lease_ttl_secs: Some(3),
        lease_renew_secs: Some(1),
        lease_owner: Some(owner.to_string()),
        ..DbConfig::default()
    }
}

/// An engine config with access control off, so the test drives the engine the
/// way a local deployment does.
fn engine_config() -> acts::Config {
    let table: toml::Table = toml::from_str("[acl]\nenabled = false\n").unwrap();
    acts::Config {
        data: Default::default(),
        table,
    }
}

fn model(id: &str) -> Workflow {
    Workflow::new()
        .with_id(id)
        .with_step(|step| step.with_id("step1"))
}

/// A live engine writes through the lease; once another instance takes the
/// database over, its writes are refused (`LeaseLost`), nothing of a refused
/// write lands, and its shutdown is triggered — while the new holder's lease
/// survives the old instance's release.
#[tokio::test(flavor = "multi_thread")]
async fn taking_the_lease_over_stops_the_stale_engine() {
    let dir = scratch("takeover");
    let db = db_config(&dir, "first");
    let opened = open_store(&dir, &db).await.unwrap();
    let lease = opened.lease().expect("the lease is on by default").clone();
    assert_eq!(lease.fence(), 1);

    let engine = engine_builder(
        &engine_config(),
        opened.store.clone(),
        &ServerPlugins::default(),
    )
    .unwrap()
    .start()
    .await
    .unwrap();
    let keeper = opened.keep_lease(&engine).expect("a lease to keep");

    // While it holds the lease, the engine's durable work goes through the
    // fence and commits.
    let executor = engine.executor(&Principal::unrestricted());
    assert!(executor.model().deploy(&model("held"), None).await.unwrap());
    let mut vars = Vars::new();
    vars.set("pid", "leaseheld");
    executor.proc().start("held", vars).await.unwrap();
    assert!(executor.model().get("held", "yml").await.is_ok());

    // A second instance takes the database over: the lease row another
    // process's `acquire` writes once the first one's TTL passed unrenewed.
    // Written through a plain handle, exactly as a takeover leaves it.
    let thief = acts_store::SqliteStore::open(db.database_url.as_deref().unwrap())
        .await
        .unwrap();
    let taken = serde_json::to_vec(&acts::LeaseRecord {
        owner: "second".to_string(),
        fence: 2,
        expires_at: i64::MAX,
    })
    .unwrap();
    thief.put(acts::LEASE_KEY, taken.clone()).await.unwrap();

    // The keeper's next renewal finds the row is not ours: the lease is lost
    // and the engine is asked to stop.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !lease.is_lost() {
        assert!(
            Instant::now() < deadline,
            "the keeper must notice the takeover within a renewal interval"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        engine.shutdown_token().is_cancelled(),
        "losing the lease stops the engine"
    );

    // Every admission that has to persist something is refused from here on —
    // the model and the process of a refused write are not in the database.
    for result in [
        executor
            .model()
            .deploy(&model("after"), None)
            .await
            .map(|_| ()),
        executor.proc().start("held", Vars::new()).await.map(|_| ()),
    ] {
        let err = result.expect_err("a stale instance must not write");
        assert!(matches!(err, ActError::LeaseLost), "got: {err}");
    }
    assert!(
        executor.model().get("after", "yml").await.is_err(),
        "the refused deploy stored nothing"
    );

    // A graceful stop of the old instance must not hand the successor's lease
    // away: its release is guarded by its own row.
    keeper.stop().await;
    assert_eq!(
        thief.one(acts::LEASE_KEY).await.unwrap(),
        Some(taken),
        "the stale instance's release left the new holder's lease alone"
    );

    engine.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two servers, one database, in sequence: the second cannot start while the
/// first holds the lease, and starts at a higher fence once the first stops —
/// which is the rolling-restart path, and what keeps a restarted instance's
/// writes distinguishable from the ones that came before it.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_takes_the_lease_the_previous_instance_released() {
    let dir = scratch("restart");
    let db = db_config(&dir, "first");
    let opened = open_store(&dir, &db).await.unwrap();
    let engine = engine_builder(
        &engine_config(),
        opened.store.clone(),
        &ServerPlugins::default(),
    )
    .unwrap()
    .start()
    .await
    .unwrap();
    let keeper = opened.keep_lease(&engine).expect("a lease to keep");
    assert_eq!(opened.lease().unwrap().fence(), 1);

    // A server started while the first runs is refused, and told who holds it.
    let blocked = open_store(&dir, &db_config(&dir, "second")).await;
    let err = match blocked {
        Ok(_) => panic!("a second server must not start on a leased database"),
        Err(err) => err,
    };
    assert!(matches!(err, ActError::LeaseHeld(_)), "got: {err}");
    assert!(err.to_string().contains("owner=first"), "got: {err}");

    // Shut the first down the way the server does: close, then stop the keeper
    // (which releases the lease, so the successor need not wait out the TTL).
    engine.close().await;
    keeper.stop().await;

    let second = open_store(&dir, &db_config(&dir, "second")).await.unwrap();
    assert_eq!(
        second.lease().unwrap().fence(),
        2,
        "the fence continues across the restart"
    );
    second.lease().unwrap().release().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
