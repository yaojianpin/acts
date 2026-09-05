use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use std::fmt::Debug;
use tracing::trace;

use crate::store::KvStore;
use crate::{
    ActError, Result, Trigger, Workflow,
    scheduler::{Process, Task},
    store::{Model, Package},
    utils,
};

use super::{
    DbCollection, DbCollectionIden, StoreBatchOp, StoreIden, collection::KvCollection, data,
};

pub struct Store {
    kv: Arc<dyn KvStore>,
}

impl Store {
    pub fn new(kv: Arc<dyn KvStore>) -> Self {
        Self { kv }
    }

    fn collection<DATA>(&self) -> Arc<dyn DbCollection<Item = DATA>>
    where
        DATA:
            DbCollectionIden + Serialize + DeserializeOwned + Send + Sync + Clone + Debug + 'static,
    {
        let prefix = DATA::iden().as_ref().to_string();
        Arc::new(KvCollection::new(&prefix, self.kv.clone()))
    }

    pub fn tasks(&self) -> Arc<dyn DbCollection<Item = data::Task>> {
        self.collection()
    }

    pub fn procs(&self) -> Arc<dyn DbCollection<Item = data::Proc>> {
        self.collection()
    }

    pub fn packages(&self) -> Arc<dyn DbCollection<Item = data::Package>> {
        self.collection()
    }

    pub fn models(&self) -> Arc<dyn DbCollection<Item = data::Model>> {
        self.collection()
    }

    pub fn messages(&self) -> Arc<dyn DbCollection<Item = data::Message>> {
        self.collection()
    }

    pub fn deliveries(&self) -> Arc<dyn DbCollection<Item = data::Delivery>> {
        self.collection()
    }

    pub fn events(&self) -> Arc<dyn DbCollection<Item = data::Event>> {
        self.collection()
    }
    pub fn ops(&self) -> Arc<dyn DbCollection<Item = data::Op>> {
        self.collection()
    }

    async fn rebuild_one<DATA>(&self) -> Result<usize>
    where
        DATA:
            DbCollectionIden + Serialize + DeserializeOwned + Send + Sync + Clone + Debug + 'static,
    {
        let prefix = DATA::iden().as_ref().to_string();
        KvCollection::<DATA>::new(&prefix, self.kv.clone())
            .rebuild_index()
            .await
    }

    /// Rebuild all collection index entries from stored data documents.
    ///
    /// Run once after upgrading to a version whose index-key value encoding
    /// changed (see `KvCollection::rebuild_index`); calling it repeatedly is
    /// harmless (idempotent rewrite).
    pub async fn rebuild_indexes(&self) -> Result<usize> {
        let mut total = 0;
        total += Self::rebuild_one::<data::Task>(self).await?;
        total += Self::rebuild_one::<data::Proc>(self).await?;
        total += Self::rebuild_one::<data::Package>(self).await?;
        total += Self::rebuild_one::<data::Model>(self).await?;
        total += Self::rebuild_one::<data::Message>(self).await?;
        total += Self::rebuild_one::<data::Delivery>(self).await?;
        total += Self::rebuild_one::<data::Event>(self).await?;
        total += Self::rebuild_one::<data::Op>(self).await?;
        Ok(total)
    }

    pub async fn publish(&self, pack: &Package) -> Result<bool> {
        trace!(id = %pack.id, "store publish");
        if pack.id.is_empty() {
            return Err(ActError::Action("missing id in package".into()));
        }

        let packages = self.packages();
        match packages.find(&pack.id).await {
            Ok(m) => {
                let data = Package {
                    create_time: m.create_time,
                    update_time: utils::time::time_millis(),
                    ..pack.clone()
                };
                packages.update(&data).await
            }
            Err(_) => {
                let data = Package {
                    create_time: utils::time::time_millis(),
                    ..pack.clone()
                };
                packages.create(&data).await
            }
        }
    }

    pub async fn deploy(&self, model: &Workflow, view: Option<&JsonValue>) -> Result<bool> {
        trace!(id = %model.id, "store deploy");
        if model.id.is_empty() {
            return Err(ActError::Model("missing id in model".into()));
        }
        if model.ver.is_empty() {
            return Err(ActError::Model("missing ver in model".into()));
        }

        // The model row, its trigger (`events`) rows and the removal of stale
        // trigger rows are committed as ONE atomic batch: a mid-deploy
        // failure can no longer leave a model row with half-reconciled (or
        // missing) triggers, or stale triggers of a removed declaration.
        let mut ops = self.model_deploy_ops(model, view).await?;
        ops.extend(self.trigger_ops(&model.on, &model.id, &model.ver).await?);
        self.kv.batch(&ops).await?;
        Ok(true)
    }

    /// KV mutations of the model row itself (create or re-deploy update) —
    /// re-deploys keep the deployed version and the original creation time.
    async fn model_deploy_ops(
        &self,
        model: &Workflow,
        view: Option<&JsonValue>,
    ) -> Result<Vec<StoreBatchOp>> {
        let models = KvCollection::<Model>::new(StoreIden::Models.as_ref(), self.kv.clone());
        let text = serde_yaml::to_string(model).unwrap();
        match self.models().find(&model.id).await {
            Ok(m) => {
                models
                    .update_ops(&Model {
                        id: model.id.clone(),
                        name: model.name.clone(),
                        desc: model.desc.clone(),
                        data: text.clone(),
                        view: view.map(|v| v.to_string()),
                        ver: m.ver.clone(),
                        size: text.len() as i32,
                        create_time: m.create_time,
                        update_time: utils::time::time_millis(),
                        timestamp: utils::time::timestamp(),
                        v: Model::version(),
                    })
                    .await
            }
            Err(_) => models.create_ops(&Model {
                id: model.id.clone(),
                name: model.name.clone(),
                desc: model.desc.clone(),
                data: text.clone(),
                view: view.map(|v| v.to_string()),
                ver: model.ver.to_string(),
                size: text.len() as i32,
                create_time: utils::time::time_millis(),
                update_time: 0,
                timestamp: utils::time::timestamp(),
                v: Model::version(),
            }),
        }
    }

    /// KV mutations that reconcile the model's `events` rows against its
    /// declared triggers (`Workflow.on`):
    ///
    /// - drop rows that are no longer declared (stale entries from an older
    ///   version of the model),
    /// - update rows whose declaration changed (name/kind/params/schedule),
    /// - create missing rows.
    ///
    /// `schedule` triggers keep their `last_run`/`next_run` state across
    /// re-deploys unless the cron expression itself changed (then the next
    /// run is re-armed to fire on the next tick).
    async fn trigger_ops(
        &self,
        triggers: &[Trigger],
        mid: &str,
        ver: &str,
    ) -> Result<Vec<StoreBatchOp>> {
        use super::query::{Expr, Filter, Query};
        use crate::utils::consts;

        let events = KvCollection::<data::Event>::new(StoreIden::Events.as_ref(), self.kv.clone());
        let existing = events
            .query(
                &Query::new()
                    .limit(1000)
                    .filter(Filter::and().expr(Expr::eq(consts::MODEL_ID, mid))),
            )
            .await?
            .rows;

        let mut ops = Vec::new();
        let mut keep = Vec::new();
        let mut declared = Vec::new();
        for trigger in triggers {
            let event_id = format!("{}:{}", mid, trigger.id);
            keep.push(event_id.clone());
            declared.push(data::Event::from_trigger(trigger, mid, ver, &event_id)?);
        }

        // rows no longer declared: drop the stale trigger rows
        for row in existing.iter() {
            if !keep.contains(&row.id) {
                ops.extend(events.delete_ops(&row.id).await?);
            }
        }

        for mut event in declared {
            match events.find(&event.id).await {
                Ok(evt) => {
                    let changed = evt.name != event.name
                        || evt.kind != event.kind
                        || evt.params != event.params
                        || evt.schedule != event.schedule
                        || evt.ver != event.ver;
                    if !changed {
                        continue;
                    }
                    // keep the schedule run state unless the cron changed
                    event.last_run = evt.last_run;
                    event.next_run = if evt.schedule == event.schedule {
                        evt.next_run
                    } else if event.schedule.is_some() {
                        utils::time::time_millis()
                    } else {
                        0
                    };
                    ops.extend(events.update_ops(&event).await?);
                }
                Err(_) => {
                    // new trigger: arm `schedule` rows on the next tick
                    if event.schedule.is_some() {
                        event.next_run = utils::time::time_millis();
                    }
                    ops.extend(events.create_ops(&event)?);
                }
            }
        }
        Ok(ops)
    }

    /// Atomically remove a model and every trigger (`events`) row of it in
    /// one batch: a mid-removal failure can no longer leave stale trigger
    /// rows (or a half-cleared event set) behind. Removing an absent model is
    /// a no-op that still returns `true`.
    pub async fn rm_model(&self, id: &str) -> Result<bool> {
        use super::query::{Expr, Filter, Query};
        use crate::utils::consts;

        let models = KvCollection::<Model>::new(StoreIden::Models.as_ref(), self.kv.clone());
        let events = KvCollection::<data::Event>::new(StoreIden::Events.as_ref(), self.kv.clone());

        let mut ops = Vec::new();
        let rows = events
            .query(&Query::new().filter(Filter::and().expr(Expr::eq(consts::MODEL_ID, id))))
            .await?
            .rows;
        for row in rows {
            ops.extend(events.delete_ops(&row.id).await?);
        }
        ops.extend(models.delete_ops(id).await?);
        self.kv.batch(&ops).await?;
        Ok(true)
    }

    /// Atomically remove a process and every row of it — task rows, durable
    /// outbox (`ops`) rows and the proc row — in one batch: a crash during
    /// removal can no longer leave a half-deleted process (some task rows
    /// gone, others + the proc row still present) that would resurrect as a
    /// broken process on the next restore. Removing an absent process is a
    /// no-op that still returns `true`.
    pub(crate) async fn remove_proc_rows(&self, pid: &str) -> Result<bool> {
        use super::query::{Expr, Filter, Query};

        let procs = KvCollection::<data::Proc>::new(StoreIden::Procs.as_ref(), self.kv.clone());
        let tasks = KvCollection::<data::Task>::new(StoreIden::Tasks.as_ref(), self.kv.clone());
        let ops = KvCollection::<data::Op>::new(StoreIden::Ops.as_ref(), self.kv.clone());

        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
        let mut batch = Vec::new();
        for row in tasks.query(&q).await?.rows {
            batch.extend(tasks.delete_ops(&row.id).await?);
        }
        for row in ops.query(&q).await?.rows {
            batch.extend(ops.delete_ops(&row.id).await?);
        }
        batch.extend(procs.delete_ops(pid).await?);
        self.kv.batch(&batch).await?;
        Ok(true)
    }

    /// Persist a process and its root task row as ONE atomic batch (upsert:
    /// an existing row is updated, a missing row is created). The first
    /// persist of a freshly started process goes through here, so a crash can
    /// never leave a durable proc row without its root task row (or a root
    /// task row whose proc row is missing — the writer skips such orphans).
    pub(crate) async fn upsert_proc_with_task(
        &self,
        proc: &Arc<Process>,
        root: Option<&Arc<Task>>,
    ) -> Result<()> {
        let procs = KvCollection::<data::Proc>::new(StoreIden::Procs.as_ref(), self.kv.clone());
        let tasks = KvCollection::<data::Task>::new(StoreIden::Tasks.as_ref(), self.kv.clone());

        let mut ops = procs.update_ops(&proc.into_data()?).await?;
        if let Some(root) = root {
            ops.extend(tasks.update_ops(&root.into_data()?).await?);
        }
        self.kv.batch(&ops).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use crate::Workflow;
    use crate::store::query::{Expr, Filter, Query};
    use crate::store::{KvStore, MemoryStore, ScanOptions, StoreBatchOp};
    use crate::utils::consts::MODEL_ID;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Kv wrapper that counts how the store writes: a whole `deploy` (model
    /// row + trigger rows) must go through exactly one `batch` call and never
    /// through raw `put`/`delete`.
    #[derive(Default)]
    struct CountingKv {
        inner: MemoryStore,
        batches: AtomicUsize,
        puts: AtomicUsize,
        deletes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl KvStore for CountingKv {
        async fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
            self.inner.get(key).await
        }

        async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.inner.put(key, value).await
        }

        async fn delete(&self, key: &str) -> crate::Result<()> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            self.inner.delete(key).await
        }

        async fn batch(&self, ops: &[StoreBatchOp]) -> crate::Result<()> {
            self.batches.fetch_add(1, Ordering::SeqCst);
            self.inner.batch(ops).await
        }

        async fn scan_prefix(
            &self,
            key: &str,
            options: ScanOptions,
        ) -> crate::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan_prefix(key, options).await
        }
    }

    fn counting_store() -> (Arc<CountingKv>, Store) {
        let kv = Arc::new(CountingKv::default());
        let store = Store::new(kv.clone());
        (kv, store)
    }

    async fn event_rows(store: &Store, mid: &str) -> Vec<crate::store::data::Event> {
        store
            .events()
            .query(&Query::new().filter(Filter::and().expr(Expr::eq(MODEL_ID, mid.to_string()))))
            .await
            .unwrap()
            .rows
    }

    fn trigger_model(mid: &str) -> Workflow {
        Workflow::new()
            .with_id(mid)
            .with_step(|step| step.with_id("step1"))
    }

    #[tokio::test]
    async fn deploy_commits_model_and_trigger_rows_in_one_batch() {
        let (kv, store) = counting_store();
        let model = trigger_model("m1")
            .with_trigger(|t| t.with_id("t-manual").with_kind("manual"))
            .with_trigger(|t| {
                t.with_id("t-cron")
                    .with_kind("schedule")
                    .with_schedule("* * * * * *")
            });
        store.deploy(&model, None).await.unwrap();

        assert_eq!(
            kv.batches.load(Ordering::SeqCst),
            1,
            "deploy must be a single atomic batch"
        );
        assert_eq!(
            (
                kv.puts.load(Ordering::SeqCst),
                kv.deletes.load(Ordering::SeqCst)
            ),
            (0, 0),
            "deploy must not fall back to raw per-key writes"
        );

        // the model row and every trigger row are visible
        assert!(store.models().find("m1").await.is_ok());
        let rows = event_rows(&store, "m1").await;
        assert_eq!(rows.len(), 2);
        let manual = rows.iter().find(|e| e.id == "m1:t-manual").unwrap();
        assert_eq!(manual.kind, "manual");
        let cron = rows.iter().find(|e| e.id == "m1:t-cron").unwrap();
        assert_eq!(cron.kind, "schedule");
        assert!(
            cron.next_run > 0,
            "schedule trigger must be armed on deploy"
        );
    }

    #[tokio::test]
    async fn redeploy_reconciles_trigger_rows_in_one_batch() {
        let (kv, store) = counting_store();
        store
            .deploy(
                &trigger_model("m1")
                    .with_trigger(|t| t.with_id("keep").with_kind("manual"))
                    .with_trigger(|t| t.with_id("drop").with_kind("manual")),
                None,
            )
            .await
            .unwrap();
        kv.batches.store(0, Ordering::SeqCst);

        // keep: kind changed; drop: gone; added: brand new
        store
            .deploy(
                &trigger_model("m1")
                    .with_trigger(|t| t.with_id("keep").with_kind("chat"))
                    .with_trigger(|t| t.with_id("added").with_kind("manual")),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            kv.batches.load(Ordering::SeqCst),
            1,
            "redeploy must be a single atomic batch"
        );
        assert_eq!(
            (
                kv.puts.load(Ordering::SeqCst),
                kv.deletes.load(Ordering::SeqCst)
            ),
            (0, 0),
            "redeploy must not fall back to raw per-key writes"
        );

        let rows = event_rows(&store, "m1").await;
        let ids: Vec<String> = rows.iter().map(|e| e.id.clone()).collect();
        assert!(ids.contains(&"m1:keep".to_string()));
        assert!(ids.contains(&"m1:added".to_string()));
        assert!(
            !ids.contains(&"m1:drop".to_string()),
            "stale trigger row must be dropped"
        );
        assert_eq!(rows.len(), 2);
        let keep = rows.iter().find(|e| e.id == "m1:keep").unwrap();
        assert_eq!(keep.kind, "chat");
    }

    #[tokio::test]
    async fn redeploy_without_triggers_clears_event_rows_in_one_batch() {
        let (kv, store) = counting_store();
        store
            .deploy(
                &trigger_model("m1").with_trigger(|t| t.with_id("gone").with_kind("manual")),
                None,
            )
            .await
            .unwrap();
        kv.batches.store(0, Ordering::SeqCst);

        store.deploy(&trigger_model("m1"), None).await.unwrap();
        assert_eq!(
            kv.batches.load(Ordering::SeqCst),
            1,
            "deploy must be a single atomic batch"
        );
        assert_eq!(
            (
                kv.puts.load(Ordering::SeqCst),
                kv.deletes.load(Ordering::SeqCst)
            ),
            (0, 0),
            "deploy must not fall back to raw per-key writes"
        );
        assert!(
            store.models().find("m1").await.is_ok(),
            "model row survives"
        );
        assert!(
            event_rows(&store, "m1").await.is_empty(),
            "trigger rows of the bare redeploy must be dropped"
        );
    }

    #[tokio::test]
    async fn rm_model_removes_model_and_trigger_rows_in_one_batch() {
        let (kv, store) = counting_store();
        store
            .deploy(
                &trigger_model("m1")
                    .with_trigger(|t| t.with_id("t1").with_kind("manual"))
                    .with_trigger(|t| t.with_id("t2").with_kind("manual")),
                None,
            )
            .await
            .unwrap();
        kv.batches.store(0, Ordering::SeqCst);

        assert!(store.rm_model("m1").await.unwrap());
        assert_eq!(
            kv.batches.load(Ordering::SeqCst),
            1,
            "rm must be a single atomic batch"
        );
        assert_eq!(
            (
                kv.puts.load(Ordering::SeqCst),
                kv.deletes.load(Ordering::SeqCst)
            ),
            (0, 0),
            "rm must not fall back to raw per-key writes"
        );
        assert!(
            store.models().find("m1").await.is_err(),
            "model row must be gone"
        );
        assert!(
            event_rows(&store, "m1").await.is_empty(),
            "trigger rows must be gone with the model"
        );

        // removing an absent model is a no-op that still returns true
        assert!(store.rm_model("m1").await.unwrap());
    }

    async fn seed_proc_rows(store: &Store, pid: &str) {
        let now = crate::utils::time::time_millis();
        let proc = crate::store::data::Proc {
            id: pid.to_string(),
            state: "running".to_string(),
            mid: "m1".to_string(),
            name: "t".to_string(),
            start_time: now,
            end_time: 0,
            timestamp: now,
            model: "{}".to_string(),
            env: "{}".to_string(),
            err: None,
            v: 0,
        };
        store.procs().create(&proc).await.unwrap();
        for tid in ["t1", "t2"] {
            let task = crate::store::data::Task {
                id: format!("{pid}{tid}"),
                pid: pid.to_string(),
                tid: tid.to_string(),
                node_data: "{}".to_string(),
                kind: "step".to_string(),
                prev: None,
                next: Vec::new(),
                parent: None,
                name: "t".to_string(),
                state: "running".to_string(),
                data: "{}".to_string(),
                sealed: String::new(),
                err: None,
                start_time: now,
                end_time: 0,
                timestamp: now,
                v: 0,
            };
            store.tasks().create(&task).await.unwrap();
        }
        let op = crate::store::data::Op {
            id: format!("{pid}o1"),
            pid: pid.to_string(),
            tid: "t1".to_string(),
            r#type: "next".to_string(),
            status: "pending".to_string(),
            event: None,
            options: None,
            create_time: now,
            update_time: now,
            v: 0,
        };
        store.ops().create(&op).await.unwrap();
    }

    #[tokio::test]
    async fn remove_proc_removes_all_rows_of_the_process_in_one_batch() {
        let (kv, store) = counting_store();
        seed_proc_rows(&store, "p1").await;
        kv.batches.store(0, Ordering::SeqCst);

        assert!(store.remove_proc_rows("p1").await.unwrap());
        assert_eq!(
            kv.batches.load(Ordering::SeqCst),
            1,
            "remove_proc must be a single atomic batch"
        );
        assert_eq!(
            (
                kv.puts.load(Ordering::SeqCst),
                kv.deletes.load(Ordering::SeqCst)
            ),
            (0, 0),
            "remove_proc must not fall back to raw per-key writes"
        );

        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", "p1".to_string())));
        assert!(
            store.procs().find("p1").await.is_err(),
            "proc row must be gone"
        );
        assert!(
            store.tasks().query(&q).await.unwrap().rows.is_empty(),
            "task rows must be gone"
        );
        assert!(
            store.ops().query(&q).await.unwrap().rows.is_empty(),
            "outbox op rows must be gone"
        );

        // removing an absent process is a no-op that still returns true
        assert!(store.remove_proc_rows("p1").await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn process_first_persist_commits_proc_and_root_task_in_one_batch() {
        let kv = Arc::new(CountingKv::default());
        let engine = crate::Engine::new()
            .set_store(Some(kv.clone()))
            .start()
            .await
            .unwrap();
        let rt = engine.runtime().clone();

        let proc = rt.create_proc("p1", &trigger_model("m1"));
        // scope the tree read guard: it must not live across the awaits below
        let root = proc
            .tree()
            .root
            .clone()
            .expect("workflow root node");
        let task = proc.create_task(&root, None).unwrap();

        kv.batches.store(0, Ordering::SeqCst);
        rt.cache().start_proc(&proc, Some(&task)).await.unwrap();

        assert_eq!(
            kv.batches.load(Ordering::SeqCst),
            1,
            "first persist must be a single atomic batch"
        );
        assert_eq!(
            (
                kv.puts.load(Ordering::SeqCst),
                kv.deletes.load(Ordering::SeqCst)
            ),
            (0, 0),
            "first persist must not fall back to raw per-key writes"
        );

        // proc row and root task row exist together — never one without the other
        let store = rt.cache().store();
        assert!(
            store.procs().find("p1").await.is_ok(),
            "proc row must exist"
        );
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", "p1".to_string())));
        let rows = store.tasks().query(&q).await.unwrap().rows;
        assert_eq!(rows.len(), 1, "root task row must exist with the proc row");
        assert_eq!(rows[0].tid, "$", "the single row is the root task");
    }
}
