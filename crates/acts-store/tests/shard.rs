use acts::{KvStore, MemoryStore, ScanOperation, ScanOptions, Store, StoreBatchOp, StoreGuard};
#[cfg(feature = "sled")]
use acts_store::SledStore;
use acts_store::{Shard, ShardConfig, ShardedStore};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[macro_use]
mod common;

fn stores(count: usize) -> Vec<Arc<MemoryStore>> {
    (0..count).map(|_| Arc::new(MemoryStore::new())).collect()
}

fn sharded(backends: &[Arc<MemoryStore>], config: ShardConfig) -> Arc<ShardedStore> {
    let shards = backends
        .iter()
        .enumerate()
        .map(|(index, store)| {
            let store: Arc<dyn KvStore> = store.clone();
            Shard::new(format!("shard-{index}"), store)
        })
        .collect();
    Arc::new(ShardedStore::with_config(shards, config).unwrap())
}

/// One data row plus its pid/state index rows, exactly the batch a task row
/// write produces.
fn batch_ops(pid: &str, rows: &[(&str, &str)]) -> Vec<StoreBatchOp> {
    let mut ops = Vec::new();
    for (tid, state) in rows {
        let id = format!("{pid}{tid}");
        ops.push(StoreBatchOp::Put {
            key: format!("tasks-id-{id}"),
            value: serde_json::json!({ "id": id, "pid": pid, "state": state })
                .to_string()
                .into_bytes(),
        });
        ops.push(StoreBatchOp::Put {
            key: format!("tasks-pid-{pid}-{id}"),
            value: Vec::new(),
        });
        ops.push(StoreBatchOp::Put {
            key: format!("tasks-state-{state}-{id}"),
            value: Vec::new(),
        });
    }
    ops
}

async fn engine_store() -> Option<Arc<Store>> {
    let backends = stores(4);
    let store = sharded(&backends, ShardConfig::default());
    Some(Arc::new(Store::new(store)))
}

gen_store_tests!(async { engine_store().await });

gen_guarded_batch_tests!({
    let backends = stores(3);
    let first: Arc<dyn KvStore> = sharded(&backends, ShardConfig::default());
    let second: Arc<dyn KvStore> = sharded(&backends, ShardConfig::default());
    (first, second)
});

gen_kv_tests!(async {
    let backends = stores(3);
    sharded(&backends, ShardConfig::default())
});

#[tokio::test]
async fn the_ring_has_1024_virtual_nodes_and_moves_a_minority_on_add() {
    let three = stores(3);
    let before = sharded(&three, ShardConfig::default());
    assert_eq!(before.virtual_nodes(), 1024);

    let four = stores(4);
    let after = sharded(&four, ShardConfig::default());

    let mut moved = 0;
    let mut counts = [0usize; 4];
    for index in 0..10_000 {
        let pid = format!("pid{index}");
        let from = format!("shard-{}", before.shard_for_pid(&pid));
        let to = after.shard_for_pid(&pid);
        counts[to] += 1;
        if from != format!("shard-{to}") {
            moved += 1;
        }
    }
    // The existing three ids keep their slots; only the new shard's slots
    // (about a quarter of the ring) attract keys.
    assert!(moved > 500, "adding a shard must move some keys: {moved}");
    assert!(
        moved < 5_000,
        "adding a fourth shard must move a minority, not rewrite the ring: {moved}"
    );
    for count in counts {
        assert!(count > 1_500, "the ring spreads pids: {counts:?}");
    }

    // Reordering the same shards must not change any pid's owner.
    let mut reordered = three.clone();
    reordered.reverse();
    let again = sharded(&reordered, ShardConfig::default());
    for pid in ["p-1", "p-2", "process-with-a-long-name"] {
        let id = |index: usize| format!("shard-{index}");
        assert_eq!(
            id(before.shard_for_pid(pid)),
            id(again.shard_for_pid(pid)),
            "the shard id, not its list position, is the ring identity"
        );
    }
}

#[tokio::test]
async fn a_process_batch_stays_on_one_shard_and_scans_merge_in_key_order() {
    let backends = stores(4);
    let store = sharded(&backends, ShardConfig::default());
    let pids = ["procalpha", "procb", "procg", "procd"];
    for (index, pid) in pids.iter().enumerate() {
        let rows = [
            ("t1", "Running"),
            (
                "t2",
                if index % 2 == 0 {
                    "Completed"
                } else {
                    "Running"
                },
            ),
        ];
        store.batch(&batch_ops(pid, &rows), &[]).await.unwrap();
        assert_eq!(
            store.shard_for_key(&format!("tasks-id-{pid}t1")),
            Some(store.shard_for_pid(pid)),
            "a data row follows its pid's shard"
        );
    }

    // Eq on one process's pid index: only that process's rows, in key order.
    let rows = store
        .scan_prefix(
            "tasks-pid-procalpha-",
            ScanOptions::new(ScanOperation::Eq, "tasks-pid-".to_string(), false),
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));

    // A range that spans every shard is merged into one global key order and
    // a reverse scan is the exact reverse of that order.
    let options = || {
        ScanOptions::new(
            ScanOperation::Range {
                lower: Some("tasks-id-procalpha".to_string()),
                upper: Some("tasks-id-procg".to_string()),
            },
            "tasks-id-".to_string(),
            false,
        )
    };
    let forward = store.scan_prefix("tasks-id-", options()).await.unwrap();
    let keys: Vec<String> = forward.iter().map(|(key, _)| key.clone()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "the merged scan is in global key order");
    assert_eq!(
        keys,
        [
            "tasks-id-procalphat1",
            "tasks-id-procalphat2",
            "tasks-id-procbt1",
            "tasks-id-procbt2",
            "tasks-id-procdt1",
            "tasks-id-procdt2",
        ]
    );

    let mut reversed = options();
    reversed.is_rev = true;
    let backward = store.scan_prefix("tasks-id-", reversed).await.unwrap();
    let mut expected = keys;
    expected.reverse();
    assert_eq!(
        backward
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
        expected
    );
}

#[tokio::test]
async fn a_batch_spanning_two_shards_is_refused_instead_of_half_applied() {
    let backends = stores(3);
    let store = sharded(&backends, ShardConfig::default());
    let first = (0..100)
        .map(|index| format!("pid{index}"))
        .find(|pid| store.shard_for_pid(pid) == 0)
        .unwrap();
    let second = (0..100)
        .map(|index| format!("pid{index}"))
        .find(|pid| store.shard_for_pid(pid) == 1)
        .unwrap();
    let mut ops = batch_ops(&first, &[("t1", "Running")]);
    ops.extend(batch_ops(&second, &[("t1", "Running")]));
    let error = store.batch(&ops, &[]).await.unwrap_err();
    assert!(
        error.to_string().contains("across physical shards"),
        "got: {error}"
    );

    // Neither document was written.
    assert_eq!(
        store.one(&format!("tasks-id-{first}t1")).await.unwrap(),
        None
    );
    assert_eq!(
        store.one(&format!("tasks-id-{second}t1")).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_guarded_batch_with_process_data_keeps_its_guard_on_the_same_shard() {
    let backends = stores(4);
    let store = sharded(&backends, ShardConfig::default());
    let ops = batch_ops("procguarded", &[("t1", "Created")]);
    assert!(store.batch(&ops, &[]).await.unwrap());
    let data_key = "tasks-id-procguardedt1";
    let before = store.one(data_key).await.unwrap().unwrap();

    let update = batch_ops("procguarded", &[("t1", "Running")]);
    let applied = store
        .batch(&update, &[StoreGuard::holds(data_key, before.clone())])
        .await
        .unwrap();
    assert!(applied);
    let update = batch_ops("procguarded", &[("t1", "Completed")]);
    let stale = store
        .batch(&update, &[StoreGuard::holds(data_key, before)])
        .await
        .unwrap();
    assert!(!stale, "the guard read and the data key share a shard");
}

#[tokio::test]
async fn rebalance_copies_before_deleting_and_draining_reads_keep_working() {
    let mut backends = stores(3);
    let old = sharded(&backends, ShardConfig::default());

    let pids: Vec<String> = (0..200).map(|index| format!("pid{index}")).collect();
    for pid in &pids {
        old.batch(&batch_ops(pid, &[("t1", "Running")]), &[])
            .await
            .unwrap();
    }
    old.put("models-id-global-model", b"global".to_vec())
        .await
        .unwrap();

    // Add a fourth physical shard. Before rebalance, scans already see every
    // shard and point reads fall back to the old owner.
    backends.push(Arc::new(MemoryStore::new()));
    let added = sharded(&backends, ShardConfig::default());
    let all = added
        .scan_prefix(
            "",
            ScanOptions::new(ScanOperation::Eq, String::new(), false),
        )
        .await
        .unwrap();
    assert!(all.iter().any(|(key, _)| key == "models-id-global-model"));
    for pid in pids.iter().take(10) {
        assert!(
            added
                .one(&format!("tasks-id-{pid}t1"))
                .await
                .unwrap()
                .is_some(),
            "a point read falls back to the pre-rebalance owner"
        );
    }

    let report = added.rebalance().await.unwrap();
    assert_eq!(report.scanned, all.len());
    assert!(
        report.copied > 0,
        "the new shard takes ownership of some keys"
    );
    assert!(report.removed > 0, "stale copies are removed");

    // Every key now lives on exactly its ring owner.
    for (shard_index, backend) in backends.iter().enumerate() {
        let rows = backend
            .scan_prefix(
                "",
                ScanOptions::new(ScanOperation::Eq, String::new(), false),
            )
            .await
            .unwrap();
        for (key, _) in rows {
            assert_eq!(
                added
                    .shard_for_key(&key)
                    .map(|index| format!("shard-{index}")),
                Some(format!("shard-{shard_index}")),
                "{key} lives on a non-owner shard"
            );
        }
    }

    // Remove a shard: keep it as a draining source, move its rows, then read
    // without it.
    let removed_index = backends.len() - 1;
    let removed = backends.pop().unwrap();
    let mut with_drain = backends
        .iter()
        .enumerate()
        .map(|(index, store)| {
            let store: Arc<dyn KvStore> = store.clone();
            Shard::new(format!("shard-{index}"), store)
        })
        .collect::<Vec<_>>();
    with_drain.push(Shard::draining(
        format!("shard-{removed_index}"),
        removed.clone(),
    ));
    let draining = Arc::new(ShardedStore::with_config(with_drain, ShardConfig::default()).unwrap());
    // The removed shard is outside the ring but still readable.
    assert!(draining.shard_count() > draining.active_shard_count());
    assert_eq!(
        draining.one("models-id-global-model").await.unwrap(),
        Some(b"global".to_vec())
    );
    draining.rebalance().await.unwrap();
    let left = removed
        .scan_prefix(
            "",
            ScanOptions::new(ScanOperation::Eq, String::new(), false),
        )
        .await
        .unwrap();
    assert!(
        left.is_empty(),
        "the draining shard has nothing left: {left:?}"
    );

    let final_store = sharded(&backends, ShardConfig::default());
    assert_eq!(
        final_store.one("models-id-global-model").await.unwrap(),
        Some(b"global".to_vec())
    );
}

/// A store whose first write parks until the test releases it. The shard's
/// write gate is then full, which is what `KvStore::saturated` must report.
struct BlockingStore {
    inner: MemoryStore,
    parked: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl BlockingStore {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            parked: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }

    async fn block_first(&self) {
        if !self.parked.swap(true, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }
}

#[async_trait::async_trait]
impl KvStore for BlockingStore {
    async fn one(&self, key: &str) -> acts::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> acts::Result<()> {
        self.block_first().await;
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> acts::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> acts::Result<bool> {
        self.block_first().await;
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> acts::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// The sharded wrapper is backend-generic: two independent sled directories
/// are two shards, and a pid's data/index batch still lands on one of them.
#[cfg(feature = "sled")]
#[tokio::test]
async fn sled_directories_work_as_physical_shards() {
    let backends: Vec<Arc<dyn KvStore>> = (0..2)
        .map(|_| Arc::new(SledStore::open_in_memory().unwrap()) as Arc<dyn KvStore>)
        .collect();
    let shards = backends
        .iter()
        .enumerate()
        .map(|(index, store)| Shard::new(format!("sled-shard-{index}"), store.clone()))
        .collect();
    let store = ShardedStore::with_config(shards, ShardConfig::default()).unwrap();

    for pid in ["sledpid0", "sledpid1", "sledpid2"] {
        store
            .batch(&batch_ops(pid, &[("t1", "Running")]), &[])
            .await
            .unwrap();
        assert_eq!(
            store.shard_for_key(&format!("tasks-id-{pid}t1")),
            Some(store.shard_for_pid(pid))
        );
    }

    let rows = store
        .scan_prefix(
            "tasks-id-",
            ScanOptions::new(ScanOperation::Eq, "tasks-id-".to_string(), false),
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let mut keys = rows.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "tasks-id-sledpid0t1",
            "tasks-id-sledpid1t1",
            "tasks-id-sledpid2t1"
        ]
    );
}

#[tokio::test]
async fn a_full_shard_gate_reports_saturation_to_the_engine() {
    let backend = Arc::new(BlockingStore::new());
    let store = Arc::new(
        ShardedStore::with_config(
            vec![Shard::new("only", backend.clone())],
            ShardConfig {
                max_in_flight_writes: 1,
                ..ShardConfig::default()
            },
        )
        .unwrap(),
    );
    assert!(!store.saturated());

    let first = {
        let store = store.clone();
        tokio::spawn(async move { store.put("model-1", b"one".to_vec()).await })
    };
    backend.entered.notified().await;
    assert!(store.saturated(), "the one write slot is held");

    // A second write waits for the gate instead of entering the backend.
    let second = {
        let store = store.clone();
        tokio::spawn(async move { store.put("model-2", b"two".to_vec()).await })
    };
    tokio::task::yield_now().await;
    assert!(store.saturated());

    backend.release.notify_one();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert!(!store.saturated(), "the released slot re-opens the shard");
    assert_eq!(store.one("model-1").await.unwrap(), Some(b"one".to_vec()));
    assert_eq!(store.one("model-2").await.unwrap(), Some(b"two".to_vec()));
}
