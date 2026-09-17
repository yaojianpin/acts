use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use std::fmt::Debug;
use tracing::trace;

use crate::store::KvStore;
use crate::{
    ActError, Result, Trigger, Workflow,
    scheduler::{Process, Task, TaskState},
    store::{Model, Package},
    utils,
};

use super::{
    DbCollection, DbCollectionIden, StoreBatchOp, StoreIden,
    collection::{DocLocks, KvCollection, lock_docs},
    data,
    data::DeliveryStatus,
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
    pub fn vars(&self) -> Arc<dyn DbCollection<Item = data::TaskVars>> {
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
        match packages.find_opt(&pack.id).await? {
            Some(m) => {
                let data = Package {
                    create_time: m.create_time,
                    update_time: utils::time::time_millis(),
                    ..pack.clone()
                };
                packages.update(&data).await
            }
            None => {
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
        //
        // The batch is a read-modify-write of the model row and of every
        // trigger row it reconciles: the model is locked first, then the
        // trigger rows it owns (`DocLocks::lock_more`), so a concurrent
        // deploy/remove of the model or a concurrent arm of one of its
        // triggers cannot interleave.
        let models = KvCollection::<Model>::new(StoreIden::Models.as_ref(), self.kv.clone());
        let mut locks = lock_docs([models.data_key(&model.id)]).await;
        let mut ops = self.model_deploy_ops(model, view).await?;
        ops.extend(
            self.trigger_ops(&model.on, &model.id, &model.ver, &mut locks)
                .await?,
        );
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
        match self.models().find_opt(&model.id).await? {
            Some(m) => {
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
            None => models.create_ops(&Model {
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
    /// run is re-armed to the new cron's next fire).
    async fn trigger_ops(
        &self,
        triggers: &[Trigger],
        mid: &str,
        ver: &str,
        locks: &mut DocLocks,
    ) -> Result<Vec<StoreBatchOp>> {
        use super::query::{Expr, Filter, Query};
        use crate::utils::consts;

        let events = KvCollection::<data::Event>::new(StoreIden::Events.as_ref(), self.kv.clone());
        // Exhaustive: the reconciliation drops every stored trigger row the
        // model no longer declares, so a row past a page limit would survive
        // as a stale trigger of a removed declaration.
        let existing = events
            .query_all(&Query::new().filter(Filter::and().expr(Expr::eq(consts::MODEL_ID, mid))))
            .await?;

        // Every trigger row this reconciliation reads (the declared ones and
        // the stale ones it drops) is written by the caller's batch: lock them
        // under the already held model lock.
        locks
            .lock_more(
                existing.iter().map(|row| events.data_key(&row.id)).chain(
                    triggers
                        .iter()
                        .map(|trigger| events.data_key(&format!("{}:{}", mid, trigger.id))),
                ),
            )
            .await;

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
            match events.find_opt(&event.id).await? {
                Some(evt) => {
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
                    } else {
                        event
                            .schedule
                            .as_deref()
                            .map(crate::scheduler::cron::Cron::next_fire_millis)
                            .unwrap_or(0)
                    };
                    ops.extend(events.update_ops(&event).await?);
                }
                None => {
                    // new trigger: arm `schedule` rows to their next cron fire
                    if let Some(schedule) = event.schedule.as_deref() {
                        event.next_run = crate::scheduler::cron::Cron::next_fire_millis(schedule);
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
        use super::query::{Expr, Filter};
        use crate::utils::consts;

        let models = KvCollection::<Model>::new(StoreIden::Models.as_ref(), self.kv.clone());
        let events = KvCollection::<data::Event>::new(StoreIden::Events.as_ref(), self.kv.clone());

        // The model row and every trigger row of it are read-modify-writes
        // committed as one batch: the model is locked first (nobody takes a
        // trigger lock before a model lock), then the trigger rows discovered
        // while that lock is held — a concurrent deploy of the same model
        // cannot add a row in between.
        let mut locks = lock_docs([models.data_key(id)]).await;
        // Exhaustive ids: a trigger row past a page limit would outlive its
        // model as an orphan.
        let row_ids = events
            .matching_ids(Some(&Filter::and().expr(Expr::eq(consts::MODEL_ID, id))))
            .await?;
        locks
            .lock_more(row_ids.iter().map(|row_id| events.data_key(row_id)))
            .await;

        let mut ops = Vec::new();
        for row_id in &row_ids {
            ops.extend(events.delete_ops(row_id).await?);
        }
        ops.extend(models.delete_ops(id).await?);
        self.kv.batch(&ops).await?;
        Ok(true)
    }

    /// Atomically remove a process and every row of it — task rows, durable
    /// outbox (`ops`) rows, the process's message (`messages`) and delivery
    /// (`deliveries`) rows and the proc row — in one batch: a crash during
    /// removal can no longer leave a half-deleted process (some task rows
    /// gone, others + the proc row still present) that would resurrect as a
    /// broken process on the next restore, nor orphaned message/delivery rows
    /// that would be retried forever after the process is gone. Removing an
    /// absent process is a no-op that still returns `true`.
    pub(crate) async fn remove_proc_rows(&self, pid: &str) -> Result<bool> {
        use super::query::{Expr, Filter};

        let procs = KvCollection::<data::Proc>::new(StoreIden::Procs.as_ref(), self.kv.clone());
        let tasks = KvCollection::<data::Task>::new(StoreIden::Tasks.as_ref(), self.kv.clone());
        let ops = KvCollection::<data::Op>::new(StoreIden::Ops.as_ref(), self.kv.clone());
        let messages =
            KvCollection::<data::Message>::new(StoreIden::Messages.as_ref(), self.kv.clone());
        let deliveries =
            KvCollection::<data::Delivery>::new(StoreIden::Deliveries.as_ref(), self.kv.clone());
        let vars = KvCollection::<data::TaskVars>::new(StoreIden::Vars.as_ref(), self.kv.clone());

        // Every row is deleted by reading the document to compute its index
        // rows: collect the ids first, then lock all of them (in key order)
        // before any of the reads, and keep the locks until the batch is
        // applied. Rows appearing after the queries are not part of this
        // removal — the writer orders it after the process's own writes and
        // the cache has evicted the process by then.
        //
        // The ids are exhaustive: a row past a page limit would survive the
        // removal as an orphan that resurrects the process or keeps retrying
        // its delivery.
        let filter = Filter::and().expr(Expr::eq("pid", pid.to_string()));
        let vars_ids = vars.matching_ids(Some(&filter)).await?;
        let task_ids = tasks.matching_ids(Some(&filter)).await?;
        let op_ids = ops.matching_ids(Some(&filter)).await?;
        let msg_ids = messages.matching_ids(Some(&filter)).await?;
        let dlv_ids = deliveries.matching_ids(Some(&filter)).await?;
        let _locks = lock_docs(
            vars_ids
                .iter()
                .map(|id| vars.data_key(id))
                .chain(task_ids.iter().map(|id| tasks.data_key(id)))
                .chain(op_ids.iter().map(|id| ops.data_key(id)))
                .chain(msg_ids.iter().map(|id| messages.data_key(id)))
                .chain(dlv_ids.iter().map(|id| deliveries.data_key(id)))
                .chain([procs.data_key(pid)]),
        )
        .await;

        let mut batch = Vec::new();
        for id in &vars_ids {
            batch.extend(vars.delete_ops(id).await?);
        }
        for id in &task_ids {
            batch.extend(tasks.delete_ops(id).await?);
        }
        for id in &op_ids {
            batch.extend(ops.delete_ops(id).await?);
        }
        for id in &msg_ids {
            batch.extend(messages.delete_ops(id).await?);
        }
        for id in &dlv_ids {
            batch.extend(deliveries.delete_ops(id).await?);
        }
        batch.extend(procs.delete_ops(pid).await?);
        self.kv.batch(&batch).await?;
        Ok(true)
    }

    /// A process may be deleted once it is finished AND every delivery of its
    /// messages has settled: a delivery is still open while it is `Created`
    /// (never successfully handed over) or `Delivered` (handed over, waiting
    /// for an ack or the task close) — delivery completion lags the process
    /// terminal state, so rows must not be deleted while any delivery is
    /// still open. Call this whenever a delivery of the process settles
    /// (acked, closed by the task, or errored out): when the process is
    /// terminal and nothing is left open it marks the proc row `removable`
    /// for the sweeper.
    pub(crate) async fn try_mark_removable(&self, pid: &str) -> Result<bool> {
        let procs = KvCollection::<data::Proc>::new(StoreIden::Procs.as_ref(), self.kv.clone());
        let Ok(proc) = procs.find(pid).await else {
            return Ok(false);
        };
        let state = TaskState::from(proc.state.as_str());
        if !state.is_completed() || proc.removable {
            return Ok(false);
        }
        if self.has_unsettled_deliveries(pid).await? {
            return Ok(false);
        }
        let mut proc = proc;
        proc.removable = true;
        procs.update(&proc).await?;
        Ok(true)
    }

    /// Whether the process still has a delivery that keeps it from being
    /// deleted. Only a row settled `Completed` (the task/message closed by
    /// the engine) allows deletion — `Acked` is just an intermediate state
    /// (the client confirmed receipt, the task may still need an action), and
    /// `Created`/`Delivered` (still being sent/retried) or `Error` (retries
    /// exhausted; only a manual resend/clear resolves it, the process must
    /// stay until then) all block it. A process with no delivery rows at all
    /// is deletable.
    async fn has_unsettled_deliveries(&self, pid: &str) -> Result<bool> {
        use super::query::{Expr, Filter, Query};
        // Exhaustive: a delivery past a page limit would be invisible to a
        // page read, and an unsettled row that is invisible lets the sweeper
        // delete a process whose delivery was still open. Read in small
        // batches and stop at the first unsettled row, so the ack path does
        // not read every delivery of the process.
        let q = Query::new()
            .limit(512)
            .filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
        Ok(self
            .deliveries()
            .find_matching(&q, &|d| d.status != DeliveryStatus::Completed)
            .await?
            .is_some())
    }

    /// Read-modify-write of delivery rows under their document locks.
    ///
    /// Every id is locked before the first read and held until the batch
    /// commits; `f` receives each stored row — the exact state the batch
    /// replaces — and returns the replacement (`None` leaves the row alone);
    /// every replacement is stamped with a new `update_time` and the whole set
    /// commits as one batch.
    ///
    /// This is what keeps the delivery transitions safe against each other:
    /// the engine's close, the retry pass and the client's delivery/ack writes
    /// reach the same rows from different tasks, and a decision taken on a
    /// stale read ("still `Delivered`, re-arm it") must never overwrite a
    /// state another transition already committed (`Completed` closed the
    /// message, `Error` exhausted its retries).
    async fn rewrite_deliveries<F>(&self, ids: &[String], mut f: F) -> Result<Vec<data::Delivery>>
    where
        F: FnMut(data::Delivery) -> Option<data::Delivery>,
    {
        let deliveries =
            KvCollection::<data::Delivery>::new(data::Delivery::iden().as_ref(), self.kv.clone());
        // Locked before the read: a concurrent transition of one of these rows
        // waits here, so the row `f` decides on is the row the batch below
        // commits over.
        let _locks = lock_docs(ids.iter().map(|id| deliveries.data_key(id))).await;

        let mut written = Vec::new();
        let mut ops = Vec::new();
        for id in ids {
            let Some(stored) = deliveries.find_opt(id).await? else {
                continue;
            };
            let Some(mut next) = f(stored) else {
                continue;
            };
            next.update_time = utils::time::time_millis();
            ops.extend(deliveries.update_ops(&next).await?);
            written.push(next);
        }
        if !ops.is_empty() {
            self.kv.batch(&ops).await?;
        }
        Ok(written)
    }

    /// [`Store::rewrite_deliveries`] for one row: the written replacement, or
    /// `None` when the row is absent or `f` declined it.
    pub(crate) async fn update_delivery<F>(&self, id: &str, f: F) -> Result<Option<data::Delivery>>
    where
        F: FnMut(data::Delivery) -> Option<data::Delivery>,
    {
        let ids = [id.to_string()];
        Ok(self.rewrite_deliveries(&ids, f).await?.pop())
    }

    /// The delivery ids of one task — exhaustive, so a row past a page limit
    /// cannot survive a transition of the task as a row nothing ever settles.
    async fn task_delivery_ids(&self, pid: &str, tid: &str) -> Result<Vec<String>> {
        use super::query::{Expr, Filter, Query};
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        self.deliveries().matching_ids(q.filter.as_ref()).await
    }

    /// Close the engine-owned delivery rows of a task: `Created`, `Delivered`
    /// and `Acked` become `Completed` — the client is not asked to act on a
    /// finished task. An `Error` row is left alone: its retries were exhausted
    /// before the task closed, only a manual resend/clear resolves it
    /// (`resend_error_deliveries`/`clear_error_deliveries`), and erasing it
    /// would drop the failed delivery from view and let the process be swept
    /// while the client never received the message.
    ///
    /// A concurrent transition of a row — the retry pass marking it `Error`,
    /// the client acking it — either lands before this read (the row is
    /// skipped or closed from its actual state) or after this batch (it sees
    /// `Completed`), never between the check and the write.
    pub(crate) async fn close_deliveries(&self, pid: &str, tid: &str) -> Result<()> {
        let ids = self.task_delivery_ids(pid, tid).await?;
        self.rewrite_deliveries(&ids, |mut delivery| {
            matches!(
                delivery.status,
                DeliveryStatus::Created | DeliveryStatus::Delivered | DeliveryStatus::Acked
            )
            .then(|| {
                delivery.status = DeliveryStatus::Completed;
                delivery
            })
        })
        .await?;
        Ok(())
    }

    /// Store the canonical row of one message together with one channel's
    /// delivery row for it.
    ///
    /// The caller writes this on the process's own writer shard
    /// ([`WriteOp::StoreDelivery`](crate::cache::writer::WriteOp)), which is
    /// what makes the two decisions below decidable: a task write (its close
    /// included), a removal, and this create are applied in enqueue order, so
    /// there is no window in which the row can slip past the close that should
    /// have settled it.
    ///
    /// - the process row is gone: its rows were already swept, and a delivery
    ///   created now would belong to a process nothing ever sweeps again — it
    ///   is dropped (the message is delivered without a delivery row) instead
    ///   of re-creating rows behind the removal;
    /// - the task is already closed: the close (`close_deliveries`) settled
    ///   every delivery of that task, and this row reached the store after it.
    ///   Nothing else ever settles a delivery of a finished task, so it is
    ///   born `Completed` — created open it would keep the process's rows
    ///   alive forever (the sweeper needs every delivery settled);
    /// - otherwise it is born `Created` for the client to ack.
    ///
    /// A message emitted outside a process (`Emitter::emit_message` from an
    /// embedder or a transport) names neither process nor task: there is no
    /// lifecycle to be ordered against, so it is stored the way it always was.
    pub(crate) async fn store_message_delivery(
        &self,
        message: &data::Message,
        delivery: &data::Delivery,
    ) -> Result<bool> {
        let carries_task = !delivery.pid.is_empty() && !delivery.tid.is_empty();
        if carries_task && !self.procs().exists(&delivery.pid).await? {
            return Ok(false);
        }
        if !self.messages().exists(&message.id).await? {
            self.messages().create(message).await?;
        }
        let mut delivery = delivery.clone();
        if carries_task && self.task_closed(&delivery.pid, &delivery.tid).await? {
            delivery.status = DeliveryStatus::Completed;
        }
        self.deliveries().create(&delivery).await?;
        Ok(true)
    }

    /// Whether the task `tid` of `pid` is already in a terminal state — the
    /// state a delivery must be born in when it reaches the store after that
    /// task closed.
    ///
    /// A row that is not there at all is NOT closed: a task admitted while the
    /// write path was saturated has no durable row yet (its own write was
    /// refused and only the `Exec` overflow descriptor was queued), and
    /// absence says nothing about its state — the client must still be able to
    /// ack a delivered message. Absence caused by a removal is the caller's
    /// process check, which the shared shard keeps in the order of the removal
    /// itself.
    async fn task_closed(&self, pid: &str, tid: &str) -> Result<bool> {
        let id = utils::Id::new(pid, tid).id();
        match self.tasks().find(&id).await {
            Ok(task) => Ok(TaskState::from(task.state.as_str()).is_completed()),
            Err(_) => Ok(false),
        }
    }

    /// Collect deliveries with no response: re-arm the ones that were handed
    /// over but never acked (`Delivered` — as well as `Created` rows that were
    /// never successfully dispatched) and mark the ones that exceeded
    /// `max_delivery_retry_times` as errors. Returns every re-armed delivery
    /// (the caller re-sends them to their own channels).
    ///
    /// The candidate page is a hint, not the decision: every row is re-read
    /// under its document lock and acted on from that stored state, so a row
    /// the engine closed, the client acked or a manual resend re-armed while
    /// this pass runs is left alone instead of being overwritten by a stale
    /// read and re-sent after the message already settled.
    pub async fn with_no_response_deliveries(
        &self,
        timeout_millis: i64,
        max_delivery_retry_times: i32,
    ) -> Result<Vec<data::Delivery>> {
        use super::query::{Expr, Filter, Query};

        // One page per pass: the caller re-sends this batch, the next tick
        // takes the following one.
        let stale_before = utils::time::time_millis() - timeout_millis;
        let q = Query::new()
            .limit(300)
            .filter(Filter::and().expr(Expr::lt("update_time", stale_before)));
        let mut ids = self.deliveries().matching_ids(q.filter.as_ref()).await?;
        ids.truncate(q.limit);

        let mut rearmed = Vec::new();
        self.rewrite_deliveries(&ids, |mut delivery| {
            // only rows that still need a response: never successfully
            // dispatched (`Created`) or handed over but not acked/closed
            // (`Delivered`); settled ones are skipped
            if !matches!(
                delivery.status,
                DeliveryStatus::Created | DeliveryStatus::Delivered
            ) {
                return None;
            }
            // the row was written again since the candidate scan (a manual
            // resend reset it, a dispatch just delivered it): it is not
            // overdue anymore — leave it to the next timeout window instead of
            // re-sending a message the client was just handed
            if delivery.update_time >= stale_before {
                return None;
            }
            if delivery.retry_times < max_delivery_retry_times {
                delivery.retry_times += 1;
                rearmed.push(delivery.clone());
            } else {
                // the delivery will re-send by manual through the manager
                // command — an errored delivery keeps its process alive
                // until a manual resend/clear resolves it
                delivery.status = DeliveryStatus::Error;
            }
            Some(delivery)
        })
        .await?;
        Ok(rearmed)
    }

    /// The sweeper pass over finished processes. Deletion is decided ONLY by
    /// the `removable` mark on the proc row — nothing else is consulted here:
    /// a process is marked removable when it is finished and every delivery
    /// settled `Completed` (see `try_mark_removable`, invoked when a delivery
    /// settles); an `Error` delivery keeps the process alive until a manual
    /// resend/clear, so it is never marked. Finished processes that are not
    /// (yet) marked are left alone — no delivery rows are read or rewritten,
    /// and no error lookup is needed. Returns the ids of the marked processes
    /// in one query over the terminal states; the CALLER deletes them
    /// (through the writer, so removal is ordered after any still-queued
    /// writes of the process) and evicts them from memory.
    pub(crate) async fn sweep_settled_procs(&self, limit: usize) -> Result<Vec<String>> {
        use super::query::{Expr, Filter, Query};
        use crate::scheduler::TaskState;

        let procs = KvCollection::<data::Proc>::new(StoreIden::Procs.as_ref(), self.kv.clone());
        let terminal: Vec<String> = [
            TaskState::Completed,
            TaskState::Cancelled,
            TaskState::Submitted,
            TaskState::Backed,
            TaskState::Error,
            TaskState::Skipped,
            TaskState::Aborted,
            TaskState::Removed,
        ]
        .iter()
        .map(String::from)
        .collect();
        // one scan over every terminal state (`state` is indexed); the
        // `removable` filter is applied in memory because the flag is not
        // indexed
        let q = Query::new()
            .limit(limit)
            .filter(Filter::and().expr(Expr::r#in("state", terminal)));
        Ok(procs
            .query(&q)
            .await?
            .rows
            .into_iter()
            .filter(|p| p.removable)
            .map(|p| p.id)
            .collect())
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

        // Both rows are read-modify-writes committed as one batch: lock them
        // (in key order) before reading either, and hold the locks until the
        // batch is applied.
        let proc_data = proc.into_data()?;
        let task_data = root.map(|root| root.into_data()).transpose()?;
        let _locks = lock_docs(
            std::iter::once(procs.data_key(&proc_data.id))
                .chain(task_data.iter().map(|task| tasks.data_key(&task.id))),
        )
        .await;

        let mut ops = procs.update_ops(&proc_data).await?;
        if let Some(task_data) = &task_data {
            ops.extend(tasks.update_ops(task_data).await?);
        }
        self.kv.batch(&ops).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use crate::Workflow;
    use crate::store::data::DeliveryStatus;
    use crate::store::query::{Expr, Filter, Query};
    use crate::store::{KvStore, MemoryStore, ScanOptions, StoreBatchOp};
    use crate::utils::consts::MODEL_ID;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

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
        async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
            self.inner.one(key).await
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

    /// Kv wrapper whose armed operation parks until released: `put` (a
    /// delivery write in flight) or `get` (a row read in flight). Lets a test
    /// hold one delivery transition inside the store at a chosen point and run
    /// a second one against the row it holds.
    #[derive(Default)]
    struct GateKv {
        inner: MemoryStore,
        gate_put: AtomicBool,
        gate_get: AtomicBool,
        in_gate: AtomicBool,
    }

    impl GateKv {
        async fn park(&self, armed: &AtomicBool) {
            if armed.load(Ordering::SeqCst) {
                self.in_gate.store(true, Ordering::SeqCst);
                while armed.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }

        fn arm_put(&self) {
            self.gate_put.store(true, Ordering::SeqCst);
        }

        fn arm_get(&self) {
            self.gate_get.store(true, Ordering::SeqCst);
        }

        fn release(&self) {
            self.gate_put.store(false, Ordering::SeqCst);
            self.gate_get.store(false, Ordering::SeqCst);
        }

        /// Yield until the armed operation is parked inside the gate.
        async fn wait_entered(&self) {
            while !self.in_gate.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
    }

    #[async_trait::async_trait]
    impl KvStore for GateKv {
        async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
            self.park(&self.gate_get).await;
            self.inner.one(key).await
        }

        async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
            self.park(&self.gate_put).await;
            self.inner.put(key, value).await
        }

        async fn delete(&self, key: &str) -> crate::Result<()> {
            self.inner.delete(key).await
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

    /// A process is swept only once it is finished AND every delivery of its
    /// messages settled `Completed`. `Acked` is only an intermediate state and
    /// `Error` needs manual handling — both keep the process alive.
    async fn seed_terminal_proc_with_delivery(
        store: &Store,
        pid: &str,
        status: crate::store::data::DeliveryStatus,
    ) {
        let now = crate::utils::time::time_millis();
        let proc = crate::store::data::Proc {
            id: pid.to_string(),
            state: "completed".to_string(),
            mid: "m1".to_string(),
            name: "t".to_string(),
            start_time: now,
            end_time: now,
            timestamp: now,
            model: "{}".to_string(),
            env: "{}".to_string(),
            err: None,
            removable: false,
            v: 0,
        };
        store.procs().create(&proc).await.unwrap();
        let message = crate::store::data::Message {
            id: format!("{pid}m1"),
            pid: pid.to_string(),
            tid: "t1".to_string(),
            ..Default::default()
        };
        store.messages().create(&message).await.unwrap();
        let delivery = crate::store::data::Delivery {
            id: format!("{pid}d1"),
            msg_id: format!("{pid}m1"),
            pid: pid.to_string(),
            tid: "t1".to_string(),
            status,
            ..Default::default()
        };
        store.deliveries().create(&delivery).await.unwrap();
    }

    #[tokio::test]
    async fn only_completed_deliveries_mark_finished_proc_removable() {
        let (_, store) = counting_store();
        // Acked is an intermediate state: no mark, no sweep
        seed_terminal_proc_with_delivery(
            &store,
            "p-acked",
            crate::store::data::DeliveryStatus::Acked,
        )
        .await;
        // Acked is only an intermediate state: it never *marks* the process
        // removable by itself, and the sweeper only deletes marked processes
        // — an Acked row is still awaiting the engine close, so the process
        // stays
        assert!(!store.try_mark_removable("p-acked").await.unwrap());
        assert!(
            !store
                .sweep_settled_procs(10)
                .await
                .unwrap()
                .contains(&"p-acked".to_string()),
            "a process with an unclosed (Acked) delivery is not swept"
        );
        assert!(store.procs().find("p-acked").await.is_ok());

        // Error needs manual handling: keeps the process alive
        seed_terminal_proc_with_delivery(
            &store,
            "p-error",
            crate::store::data::DeliveryStatus::Error,
        )
        .await;
        assert!(!store.try_mark_removable("p-error").await.unwrap());
        assert!(
            !store
                .sweep_settled_procs(10)
                .await
                .unwrap()
                .contains(&"p-error".to_string())
        );
        assert!(
            store.procs().find("p-error").await.is_ok(),
            "an errored delivery keeps its process alive for manual handling"
        );

        // a finished process with no delivery rows at all is marked (nothing
        // blocks it) and swept
        seed_terminal_proc_with_delivery(
            &store,
            "p-none",
            crate::store::data::DeliveryStatus::Completed,
        )
        .await;
        store.deliveries().delete("p-noned1").await.unwrap();
        assert!(store.try_mark_removable("p-none").await.unwrap());
        assert!(
            store
                .sweep_settled_procs(10)
                .await
                .unwrap()
                .contains(&"p-none".to_string()),
            "a finished process without deliveries is deletable"
        );

        // all deliveries Completed: marked removable and collected by the
        // sweeper; the caller then deletes the rows (through the writer)
        seed_terminal_proc_with_delivery(
            &store,
            "p-completed",
            crate::store::data::DeliveryStatus::Completed,
        )
        .await;
        assert!(store.try_mark_removable("p-completed").await.unwrap());
        assert!(
            store
                .sweep_settled_procs(10)
                .await
                .unwrap()
                .contains(&"p-completed".to_string())
        );
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", "p-completed".to_string())));
        assert!(store.remove_proc_rows("p-completed").await.unwrap());
        assert!(store.procs().find("p-completed").await.is_err());
        assert!(store.messages().query(&q).await.unwrap().rows.is_empty());
        assert!(store.deliveries().query(&q).await.unwrap().rows.is_empty());
    }

    /// A delivery row is decided as it is written, on the process's own shard:
    /// the create is ordered with that process's task writes and its removal,
    /// so a row can never land after the close (or the removal) that should
    /// have settled or dropped it. The outcomes:
    ///
    /// - the task is still running: an open row for the client to ack;
    /// - the task closed first: born `Completed` — the close settled every
    ///   delivery of that task and nothing settles a later one, so an open row
    ///   here would keep the finished process's rows (and its pid, and its
    ///   workdir) alive forever;
    /// - the task has no durable row (admitted while the write path was
    ///   saturated): absence is not a close, the row stays open;
    /// - the message names no process at all: nothing to order it against;
    /// - the process is gone: nothing is written, so no message/delivery row
    ///   is re-created behind the removal that swept them.
    #[tokio::test]
    async fn a_delivery_is_born_settled_when_its_task_already_closed() {
        let (_, store) = counting_store();
        let now = crate::utils::time::time_millis();
        store
            .procs()
            .create(&crate::store::data::Proc {
                id: "p-born".to_string(),
                state: "running".to_string(),
                mid: "m1".to_string(),
                name: "t".to_string(),
                start_time: now,
                end_time: 0,
                timestamp: now,
                model: "{}".to_string(),
                env: "{}".to_string(),
                err: None,
                removable: false,
                v: 0,
            })
            .await
            .unwrap();
        for (tid, state) in [("t-run", "running"), ("t-done", "completed")] {
            store
                .tasks()
                .create(&crate::store::data::Task {
                    id: crate::utils::Id::new("p-born", tid).id(),
                    pid: "p-born".to_string(),
                    tid: tid.to_string(),
                    node_data: "{}".to_string(),
                    kind: "act".to_string(),
                    prev: None,
                    next: Vec::new(),
                    parent: None,
                    name: "n".to_string(),
                    state: state.to_string(),
                    err: None,
                    start_time: now,
                    end_time: 0,
                    timestamp: now,
                    v: 0,
                })
                .await
                .unwrap();
        }

        async fn store_delivery(store: &Store, tid: &str, id: &str) -> crate::Result<bool> {
            let message = crate::store::data::Message {
                id: format!("m-{id}"),
                pid: "p-born".to_string(),
                tid: tid.to_string(),
                ..Default::default()
            };
            let delivery = crate::store::data::Delivery {
                id: format!("d-{id}"),
                msg_id: format!("m-{id}"),
                pid: "p-born".to_string(),
                tid: tid.to_string(),
                ..Default::default()
            };
            store.store_message_delivery(&message, &delivery).await
        }

        // the task is still running: the row waits for the client's ack
        assert!(store_delivery(&store, "t-run", "open").await.unwrap());
        assert_eq!(
            store.deliveries().find("d-open").await.unwrap().status,
            DeliveryStatus::Created
        );

        // the task closed before the row reached the store: it is born
        // settled, with the canonical message row it carries
        assert!(store_delivery(&store, "t-done", "late").await.unwrap());
        assert_eq!(
            store.deliveries().find("d-late").await.unwrap().status,
            DeliveryStatus::Completed
        );
        assert!(store.messages().find("m-late").await.is_ok());

        // no durable task row at all (a task admitted while the write path was
        // saturated has none yet): absence is not a close — the client is
        // still asked to ack
        assert!(store_delivery(&store, "t-unwritten", "new").await.unwrap());
        assert_eq!(
            store.deliveries().find("d-new").await.unwrap().status,
            DeliveryStatus::Created
        );

        // a message emitted outside any process names no task: there is no
        // lifecycle to be ordered against, so it is stored open as before
        assert!(
            store
                .store_message_delivery(
                    &crate::store::data::Message {
                        id: "m-free".to_string(),
                        ..Default::default()
                    },
                    &crate::store::data::Delivery {
                        id: "d-free".to_string(),
                        msg_id: "m-free".to_string(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store.deliveries().find("d-free").await.unwrap().status,
            DeliveryStatus::Created
        );

        // the process's rows are gone: the message is not stored at all
        store.procs().delete("p-born").await.unwrap();
        assert!(!store_delivery(&store, "t-done", "ghost").await.unwrap());
        assert!(store.deliveries().find("d-ghost").await.is_err());
        assert!(store.messages().find("m-ghost").await.is_err());
    }

    /// The terminal delivery close settles the engine-owned rows of the task
    /// and leaves an `Error` one untouched: retries exhausted before the task
    /// finished keep the failed delivery — and its process — for manual
    /// handling, until the operator resends or clears it.
    #[tokio::test]
    async fn close_deliveries_preserves_error_rows() {
        let (_, store) = counting_store();
        seed_terminal_proc_with_delivery(
            &store,
            "p-close",
            crate::store::data::DeliveryStatus::Error,
        )
        .await;
        // a second delivery of the same task, still awaiting the client, plus
        // rows of another task and of another process that must not be touched
        for (id, pid, tid, status) in [
            (
                "p-closed2",
                "p-close",
                "t1",
                crate::store::data::DeliveryStatus::Delivered,
            ),
            (
                "p-closed3",
                "p-close",
                "t2",
                crate::store::data::DeliveryStatus::Created,
            ),
            (
                "p-closed4",
                "p-other",
                "t1",
                crate::store::data::DeliveryStatus::Created,
            ),
        ] {
            store
                .deliveries()
                .create(&crate::store::data::Delivery {
                    id: id.to_string(),
                    msg_id: "p-closem1".to_string(),
                    pid: pid.to_string(),
                    tid: tid.to_string(),
                    status,
                    ..Default::default()
                })
                .await
                .unwrap();
        }

        store.close_deliveries("p-close", "t1").await.unwrap();

        // the errored row survives; the open one is closed (`Error` is not a
        // closable state, `Delivered` is); neither the other task's row nor
        // the other process's row is touched
        assert_eq!(
            store.deliveries().find("p-closed1").await.unwrap().status,
            crate::store::data::DeliveryStatus::Error
        );
        assert_eq!(
            store.deliveries().find("p-closed2").await.unwrap().status,
            crate::store::data::DeliveryStatus::Completed
        );
        assert_eq!(
            store.deliveries().find("p-closed3").await.unwrap().status,
            crate::store::data::DeliveryStatus::Created
        );
        assert_eq!(
            store.deliveries().find("p-closed4").await.unwrap().status,
            crate::store::data::DeliveryStatus::Created
        );
        // the errored delivery keeps the process alive — no removable mark, no
        // sweep; clearing the error manually and closing the process's
        // remaining open row lets it settle
        assert!(!store.try_mark_removable("p-close").await.unwrap());
        assert!(
            !store
                .sweep_settled_procs(10)
                .await
                .unwrap()
                .contains(&"p-close".to_string())
        );
        assert!(store.procs().find("p-close").await.is_ok());
        assert!(store.clear_error_delivery("p-closed1").await.unwrap());
        store.close_deliveries("p-close", "t2").await.unwrap();
        assert!(store.try_mark_removable("p-close").await.unwrap());
    }

    /// Seed one delivery row whose last write is `age_millis` in the past.
    async fn seed_delivery(
        store: &Store,
        id: &str,
        pid: &str,
        tid: &str,
        status: DeliveryStatus,
        retry_times: i32,
        age_millis: i64,
    ) {
        let now = crate::utils::time::time_millis();
        store
            .deliveries()
            .create(&crate::store::data::Delivery {
                id: id.to_string(),
                msg_id: format!("{id}m"),
                pid: pid.to_string(),
                tid: tid.to_string(),
                status,
                retry_times,
                create_time: now,
                update_time: now - age_millis,
                ..Default::default()
            })
            .await
            .unwrap();
    }

    /// The retry pass acts on the stored row, not on the candidate page it
    /// scanned: only an overdue still-open row is re-armed and returned, a row
    /// whose retries are exhausted becomes `Error` (kept for manual resend, not
    /// re-sent), and a settled (`Completed`/`Acked`) or freshly written row is
    /// left alone.
    #[tokio::test]
    async fn retry_scan_rearms_open_rows_and_errors_the_exhausted() {
        let (_, store) = counting_store();
        seed_delivery(
            &store,
            "d-open",
            "p1",
            "t1",
            DeliveryStatus::Delivered,
            0,
            10_000,
        )
        .await;
        seed_delivery(
            &store,
            "d-max",
            "p1",
            "t1",
            DeliveryStatus::Delivered,
            3,
            10_000,
        )
        .await;
        seed_delivery(
            &store,
            "d-acked",
            "p1",
            "t1",
            DeliveryStatus::Acked,
            0,
            10_000,
        )
        .await;
        seed_delivery(
            &store,
            "d-done",
            "p1",
            "t1",
            DeliveryStatus::Completed,
            0,
            10_000,
        )
        .await;
        // written a moment ago: not overdue, even though it was in the
        // candidate id set by the time the row is read
        seed_delivery(
            &store,
            "d-fresh",
            "p1",
            "t1",
            DeliveryStatus::Delivered,
            0,
            0,
        )
        .await;

        let rearmed = store.with_no_response_deliveries(1_000, 3).await.unwrap();
        let ids: Vec<&str> = rearmed.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["d-open"], "only the overdue open row is re-sent");
        assert_eq!(rearmed[0].retry_times, 1);

        let d_max = store.deliveries().find("d-max").await.unwrap();
        assert_eq!(d_max.status, DeliveryStatus::Error);
        assert_eq!(d_max.retry_times, 3, "an errored row keeps its retry count");
        assert_eq!(
            store.deliveries().find("d-acked").await.unwrap().status,
            DeliveryStatus::Acked
        );
        assert_eq!(
            store.deliveries().find("d-done").await.unwrap().status,
            DeliveryStatus::Completed
        );
        // written a moment ago: neither re-armed nor re-sent
        let fresh = store.deliveries().find("d-fresh").await.unwrap();
        assert_eq!(fresh.status, DeliveryStatus::Delivered);
        assert_eq!(fresh.retry_times, 0);
    }

    /// The retry pass holds each row's document lock across its read and its
    /// write, so a close of the same task cannot slip in between: the close
    /// waits, sees the re-armed `Delivered` row and closes it `Completed`
    /// instead of leaving the pass's stale write on top of a finished task.
    #[tokio::test(flavor = "multi_thread")]
    async fn retry_scan_does_not_overwrite_a_concurrent_close() {
        let kv = Arc::new(GateKv::default());
        let store = Arc::new(Store::new(kv.clone()));
        seed_delivery(
            &store,
            "d1",
            "p1",
            "t1",
            DeliveryStatus::Delivered,
            0,
            10_000,
        )
        .await;

        // hold the pass inside its write of d1, with the row's lock held
        kv.arm_put();
        let scan = tokio::spawn({
            let store = store.clone();
            async move { store.with_no_response_deliveries(1_000, 3).await }
        });
        kv.wait_entered().await;

        let close = tokio::spawn({
            let store = store.clone();
            async move { store.close_deliveries("p1", "t1").await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !close.is_finished(),
            "the close must wait for the delivery lock the retry pass holds"
        );

        kv.release();
        let rearmed = scan.await.unwrap().unwrap();
        close.await.unwrap().unwrap();

        assert_eq!(rearmed.len(), 1, "the pass re-armed the row it found open");
        assert_eq!(
            store.deliveries().find("d1").await.unwrap().status,
            DeliveryStatus::Completed,
            "the close decided on the stored row, so the finished task's delivery settles"
        );
    }

    /// A row rewritten between the pass's candidate scan and its row read wins:
    /// the pass reads the stored row, whose fresh `update_time` says the
    /// message was just handed over (or re-armed by an operator), so it is left
    /// to the next timeout window instead of being re-sent on the stale
    /// candidate view — no duplicate delivery, no inflated retry count.
    #[tokio::test(flavor = "multi_thread")]
    async fn retry_scan_skips_a_row_rewritten_after_its_candidate_scan() {
        use super::{KvCollection, StoreIden};

        let kv = Arc::new(GateKv::default());
        let store = Arc::new(Store::new(kv.clone()));
        seed_delivery(&store, "d1", "p1", "t1", DeliveryStatus::Created, 0, 10_000).await;

        // park the pass at its row read: the candidate ids are already
        // resolved from the index
        kv.arm_get();
        let scan = tokio::spawn({
            let store = store.clone();
            async move { store.with_no_response_deliveries(1_000, 3).await }
        });
        kv.wait_entered().await;

        // the row is written again in that window (a dispatch just succeeded,
        // a timer re-armed it): same open status, fresh `update_time`
        let now = crate::utils::time::time_millis();
        let row = crate::store::data::Delivery {
            id: "d1".to_string(),
            msg_id: "d1m".to_string(),
            pid: "p1".to_string(),
            tid: "t1".to_string(),
            status: DeliveryStatus::Created,
            retry_times: 1,
            create_time: now,
            update_time: now,
            ..Default::default()
        };
        let key = KvCollection::<crate::store::data::Delivery>::new(
            StoreIden::Deliveries.as_ref(),
            kv.clone(),
        )
        .data_key("d1");
        kv.put(&key, serde_json::to_vec(&row).unwrap())
            .await
            .unwrap();
        kv.release();

        let rearmed = scan.await.unwrap().unwrap();
        assert!(
            rearmed.is_empty(),
            "a row rewritten after the candidate scan is not re-armed from the stale view"
        );
        let row = store.deliveries().find("d1").await.unwrap();
        assert_eq!(row.status, DeliveryStatus::Created);
        assert_eq!(row.retry_times, 1, "the pass left the rewritten row alone");
    }

    /// A client ack waits for a delivery write in flight and then reads the
    /// stored state: it can no longer leave an `Acked` row behind on a
    /// delivery the engine already closed (which nothing would ever settle
    /// again — the process would stay unsettled and never be swept).
    #[tokio::test(flavor = "multi_thread")]
    async fn ack_waits_for_a_close_in_flight_and_keeps_it_closed() {
        let kv = Arc::new(GateKv::default());
        let store = Arc::new(Store::new(kv.clone()));
        seed_delivery(
            &store,
            "d1",
            "p1",
            "t1",
            DeliveryStatus::Delivered,
            0,
            10_000,
        )
        .await;

        kv.arm_put();
        let ack = tokio::spawn({
            let store = store.clone();
            async move { store.set_delivery("d1", DeliveryStatus::Acked).await }
        });
        kv.wait_entered().await;

        let close = tokio::spawn({
            let store = store.clone();
            async move { store.close_deliveries("p1", "t1").await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !close.is_finished(),
            "the close must wait for the delivery lock the ack holds"
        );

        kv.release();
        ack.await.unwrap().unwrap();
        close.await.unwrap().unwrap();
        assert_eq!(
            store.deliveries().find("d1").await.unwrap().status,
            DeliveryStatus::Completed
        );
    }

    async fn delivery_count(store: &Store, q: &Query) -> usize {
        store.deliveries().query(q).await.unwrap().rows.len()
    }

    /// `ExprOp::In` on an indexed field must behave exactly like the matching
    /// `Eq` scans: single value, multiple values, and combined with other
    /// ANDed clauses. Guards the index scan path against regressions.
    #[tokio::test]
    async fn store_in_query_matches_eq_on_indexed_field() {
        let (_, store) = counting_store();
        let delivery = crate::store::data::Delivery {
            id: "d1".to_string(),
            msg_id: "m1".to_string(),
            pid: "p1".to_string(),
            tid: "t1".to_string(),
            status: crate::store::data::DeliveryStatus::Acked,
            ..Default::default()
        };
        store.deliveries().create(&delivery).await.unwrap();

        // a second delivery with a different status so the filters are
        // discriminating
        let other = crate::store::data::Delivery {
            id: "d2".to_string(),
            msg_id: "m1".to_string(),
            pid: "p1".to_string(),
            tid: "t2".to_string(),
            status: crate::store::data::DeliveryStatus::Error,
            ..Default::default()
        };
        store.deliveries().create(&other).await.unwrap();

        let count = delivery_count;
        let eq = count(
            &store,
            &Query::new().filter(Filter::and().expr(Expr::eq(
                "status",
                crate::store::data::DeliveryStatus::Acked as i8,
            ))),
        )
        .await;
        let single = count(
            &store,
            &Query::new().filter(Filter::and().expr(Expr::r#in(
                "status",
                vec![crate::store::data::DeliveryStatus::Acked as i8],
            ))),
        )
        .await;
        let multi = count(
            &store,
            &Query::new().filter(Filter::and().expr(Expr::r#in(
                "status",
                vec![
                    crate::store::data::DeliveryStatus::Acked as i8,
                    crate::store::data::DeliveryStatus::Error as i8,
                ],
            ))),
        )
        .await;
        let with_pid = count(
            &store,
            &Query::new().filter(Filter::and().expr(Expr::eq("pid", "p1".to_string())).expr(
                Expr::r#in(
                    "status",
                    vec![crate::store::data::DeliveryStatus::Acked as i8],
                ),
            )),
        )
        .await;
        let with_range = count(
            &store,
            &Query::new().filter(
                Filter::and()
                    .expr(Expr::lt(
                        "update_time",
                        crate::utils::time::time_millis() + 1,
                    ))
                    .expr(Expr::r#in(
                        "status",
                        vec![crate::store::data::DeliveryStatus::Acked as i8],
                    )),
            ),
        )
        .await;

        assert_eq!(eq, 1, "eq finds only the Acked delivery");
        assert_eq!(single, eq, "In with one value equals the eq scan");
        assert_eq!(multi, 2, "In with both statuses finds both deliveries");
        assert_eq!(with_pid, 1, "In combined with an ANDed pid eq");
        assert_eq!(with_range, 1, "In combined with an ANDed range");

        // string values across several indexed procs — the sweeper queries
        // terminal proc states with exactly this shape
        let proc = |id: &str, state: &str| crate::store::data::Proc {
            id: id.to_string(),
            state: state.to_string(),
            mid: "m1".to_string(),
            name: "t".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: "{}".to_string(),
            env: "{}".to_string(),
            err: None,
            removable: false,
            v: 0,
        };
        // ids may contain `-` (e.g. user-supplied process ids): the index
        // scan must recover the whole id, not truncate it at the last
        // separator
        store
            .procs()
            .create(&proc("sa", "completed"))
            .await
            .unwrap();
        store.procs().create(&proc("p-2", "error")).await.unwrap();
        store.procs().create(&proc("sc", "running")).await.unwrap();

        let state_eq = store
            .procs()
            .query(
                &Query::new()
                    .filter(Filter::and().expr(Expr::eq("state", "completed".to_string()))),
            )
            .await
            .unwrap()
            .rows
            .len();
        let state_in = store
            .procs()
            .query(&Query::new().filter(Filter::and().expr(Expr::r#in(
                "state",
                vec!["completed".to_string(), "error".to_string()],
            ))))
            .await
            .unwrap()
            .rows
            .len();
        assert_eq!(state_eq, 1, "state eq finds the completed proc");
        assert_eq!(state_in, 2, "state In finds both terminal procs");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn process_first_persist_commits_proc_and_root_task_in_one_batch() {
        let kv = Arc::new(CountingKv::default());
        let engine = crate::Engine::builder()
            .set_store(kv.clone())
            .start()
            .await
            .unwrap();
        let rt = engine.runtime().clone();

        let proc = rt.create_proc("p1", &trigger_model("m1"));
        // scope the tree read guard: it must not live across the awaits below
        let root = proc.tree().root.clone().expect("workflow root node");
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

    /// Recovery and cleanup must see every row of a match set larger than one
    /// `Query::new()` page: a truncated crash-replay read silently drops
    /// outbox records, and a truncated removal leaves task/vars/outbox rows
    /// (with their index rows) behind as orphans. The set below crosses the
    /// 100000 page limit, so a limit-capped read or delete fails the count
    /// assertions.
    #[tokio::test(flavor = "multi_thread")]
    async fn recovery_and_removal_are_exhaustive_past_one_query_page() {
        use super::{KvCollection, StoreIden};
        use crate::store::data::{Op, OpStatus};

        let (kv, store) = counting_store();
        let pid = "p-huge";
        let total = 100_001usize;
        let ops = KvCollection::<Op>::new(StoreIden::Ops.as_ref(), kv.clone());
        let now = crate::utils::time::time_millis();
        // Write the seed rows in batches (`create_ops` is exactly what a
        // `create` of an absent id applies): 100001 per-row `create` calls
        // would spend most of the test in store round trips.
        let mut pending = Vec::new();
        for i in 0..total {
            pending.extend(
                ops.create_ops(&Op {
                    id: format!("{pid}op{i}"),
                    pid: pid.to_string(),
                    tid: "t1".to_string(),
                    r#type: "next".to_string(),
                    status: OpStatus::Pending.as_ref().to_string(),
                    event: None,
                    options: None,
                    create_time: now,
                    update_time: now,
                    v: 0,
                })
                .unwrap(),
            );
            if pending.len() >= 4096 * 4 {
                kv.batch(&pending).await.unwrap();
                pending.clear();
            }
        }
        if !pending.is_empty() {
            kv.batch(&pending).await.unwrap();
        }

        let filter = Filter::and().expr(Expr::eq("pid", pid.to_string()));
        // the premise: a page read stops at the limit, so an implementation
        // that used `query` here would lose rows
        let page = store
            .ops()
            .query(&Query::new().filter(filter.clone()))
            .await
            .unwrap();
        assert_eq!(page.count, total);
        assert_eq!(
            page.rows.len(),
            100_000,
            "the page limit must bound `query`"
        );

        // the replay set is the whole set, not one page of it
        assert_eq!(
            store.load_pending_ops().await.unwrap().len(),
            total,
            "crash recovery must replay every pending outbox record"
        );

        // and the removal deletes all of them: no data row and no index row
        // of the process may survive
        assert!(store.remove_proc_rows(pid).await.unwrap());
        assert!(
            store.ops().matching_ids(None).await.unwrap().is_empty(),
            "every outbox row must be removed"
        );
        assert_eq!(
            store
                .ops()
                .query(&Query::new().filter(filter))
                .await
                .unwrap()
                .count,
            0,
            "no index row of a removed outbox row may survive"
        );
    }
}
