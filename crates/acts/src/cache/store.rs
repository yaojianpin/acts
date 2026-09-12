use crate::{
    ActError, Error, Result, Workflow,
    data::{self, DeliveryStatus},
    scheduler::{self, Node, NodeData, Runtime, TaskState},
    store::{DbCollectionIden, Store, query::*},
    utils,
};
use std::{collections::HashSet, sync::Arc};
use tracing::debug;

impl Store {
    /// Load up to `cap` parked processes: durable rows in `None` state — a
    /// process was created while the resident set was full (see
    /// `Cache::admit`) or a never-started leftover from a crash — and never
    /// ran. Oldest first, so the restore pass refills slots FIFO. Parked rows
    /// are never resident, so `skip` only guards against a row whose pid is
    /// concurrently admitted (e.g. seeded by tests).
    pub async fn load_parked(
        &self,
        cap: usize,
        rt: &Arc<Runtime>,
        skip: &HashSet<String>,
    ) -> Result<Vec<Arc<scheduler::Process>>> {
        debug!("load_parked cap={}", cap);
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new()
                .filter(Filter::and().expr(Expr::eq("state", TaskState::None.to_string())))
                .order("timestamp", Sort::Asc)
                .limit(cap);
            let procs = self.procs().query(&query).await?;
            for p in procs.rows {
                if skip.contains(&p.id) {
                    continue;
                }
                let proc = self.decode_proc(p, rt).await?;
                ret.push(proc);
                if ret.len() >= cap {
                    break;
                }
            }
        }

        Ok(ret)
    }

    /// Load up to `cap` resumable processes: durable rows that were running
    /// when the engine crashed (`Ready`/`Running`/`Pending`), oldest first —
    /// the boot-resume working set. The number of matching rows beyond the
    /// cap is found with [`Self::count_resumable`]; the caller queues their
    /// pids for later slots. See `Runtime::resume`.
    pub async fn load_resumable(
        &self,
        cap: usize,
        rt: &Arc<Runtime>,
        skip: &HashSet<String>,
    ) -> Result<Vec<Arc<scheduler::Process>>> {
        debug!("load_resumable cap={}", cap);
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new()
                .filter(
                    Filter::or()
                        .expr(Expr::eq("state", TaskState::Ready.to_string()))
                        .expr(Expr::eq("state", TaskState::Running.to_string()))
                        .expr(Expr::eq("state", TaskState::Pending.to_string())),
                )
                .order("timestamp", Sort::Asc)
                .limit(cap);
            let procs = self.procs().query(&query).await?;
            for p in procs.rows {
                if skip.contains(&p.id) {
                    continue;
                }
                let proc = self.decode_proc(p, rt).await?;
                ret.push(proc);
                if ret.len() >= cap {
                    break;
                }
            }
        }
        Ok(ret)
    }

    /// Count durable rows that were in flight when the engine crashed
    /// (`Ready`/`Running`/`Pending`) — used at boot to detect processes that
    /// do not fit the resident cap and must wait in the resume queue.
    pub async fn count_resumable(&self) -> Result<usize> {
        let query = Query::new()
            .filter(
                Filter::or()
                    .expr(Expr::eq("state", TaskState::Ready.to_string()))
                    .expr(Expr::eq("state", TaskState::Running.to_string()))
                    .expr(Expr::eq("state", TaskState::Pending.to_string())),
            )
            .limit(1);
        Ok(self.procs().query(&query).await?.count)
    }

    /// Decode one durable proc row into an in-memory process (model, state,
    /// timings, env, error) and attach its persisted task graph.
    async fn decode_proc(
        &self,
        p: data::Proc,
        rt: &Arc<Runtime>,
    ) -> Result<Arc<scheduler::Process>> {
        let model = Workflow::from_json(&p.model)?;
        let env_local: serde_json::Value =
            serde_json::from_str(&p.env).map_err(|err| ActError::Store(err.to_string()))?;
        let state = p.state.clone();
        let proc = scheduler::Process::new_with_timestamp(&p.id, p.timestamp, rt);

        proc.load_owned(model)?;
        proc.set_pure_state(state.into());
        proc.set_start_time(p.start_time);
        proc.set_end_time(p.end_time);
        proc.set_env(&env_local.into());
        if let Some(err) = p.err {
            let err: Error =
                serde_json::from_str(&err).map_err(|err| ActError::Store(err.to_string()))?;
            proc.set_pure_err(&err)
        }

        self.load_tasks(&proc, rt).await?;
        Ok(proc)
    }

    pub async fn load_proc(
        &self,
        pid: &str,
        rt: &Arc<Runtime>,
    ) -> Result<Option<Arc<scheduler::Process>>> {
        debug!("load process pid={}", pid);
        match self.procs().find(pid).await {
            Ok(p) => {
                // println!("process model={}", p.model);
                let model = Workflow::from_json(&p.model)?;
                let proc = scheduler::Process::new(pid, rt);
                let env_local: serde_json::Value =
                    serde_json::from_str(&p.env).map_err(|err| ActError::Store(err.to_string()))?;

                proc.load_owned(model)?;
                proc.set_pure_state(p.state.into());
                proc.set_start_time(p.start_time);
                proc.set_env(&env_local.into());
                self.load_tasks(&proc, rt).await?;
                if let Some(err) = p.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    proc.set_pure_err(&err)
                }
                Ok(Some(proc))
            }
            Err(_) => Ok(None),
        }
    }

    pub async fn remove_proc(&self, pid: &str) -> Result<bool> {
        debug!("remove_proc pid={}", pid);
        // All rows of the process — tasks, outbox ops, its message/delivery
        // rows and the proc row — are removed as ONE atomic batch, so a crash
        // mid-removal cannot leave a half-deleted process behind nor orphaned
        // message/delivery rows that would be retried forever.
        self.remove_proc_rows(pid).await
    }

    /// but not yet run. Deduplicated per `(pid, tid, type)` — at most one
    /// in-flight record per operation, matching the previous
    /// `Sign::NEXT_PENDING` semantics. Queued on the store writer (FIFO)
    /// *before* the in-memory queue dispatch, after the task state write, so a
    /// `Pending` record always has a durable task behind it.
    pub async fn enqueue_next_op(&self, pid: &str, tid: &str) -> Result<()> {
        self.enqueue_op(pid, tid, data::OpType::Next, None, None)
            .await
    }

    /// Record a durable outbox entry for task execution. This is the disk
    /// overflow queue used when the in-memory scheduler queue is full.
    pub async fn enqueue_exec_op(&self, pid: &str, tid: &str) -> Result<()> {
        self.enqueue_op(pid, tid, data::OpType::Exec, None, None)
            .await
    }

    /// Record a durable outbox entry for a client action (event + options).
    /// Deduplicated per `(pid, tid, type)`, so it is not shadowed by the
    /// task's in-flight `next` record (an interrupt act keeps its `next` op
    /// `Pending` while waiting for the client). Written before the action is
    /// applied so recovery can re-apply it when the crash happened before the
    /// task state write became durable.
    pub async fn enqueue_action_op(
        &self,
        pid: &str,
        tid: &str,
        event: &str,
        options: &str,
    ) -> Result<()> {
        self.enqueue_op(
            pid,
            tid,
            data::OpType::Action,
            Some(event.to_string()),
            Some(options.to_string()),
        )
        .await
    }

    async fn enqueue_op(
        &self,
        pid: &str,
        tid: &str,
        r#type: data::OpType,
        event: Option<String>,
        options: Option<String>,
    ) -> Result<()> {
        let collection = self.ops();
        // Dedup against an in-flight Pending record for this (pid, tid, type).
        // The query is scoped to (pid, tid) — at most a couple of rows — with
        // the type/status filters applied in memory; a `type` or `status`
        // expression would scan every record of that type/status in the
        // collection.
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        let existing = collection.query(&q).await?;
        if existing
            .rows
            .iter()
            .any(|op| op.r#type == r#type.as_ref() && op.status == data::OpStatus::Pending.as_ref())
        {
            return Ok(());
        }

        let now = utils::time::time_millis();
        let op = data::Op {
            id: utils::longid(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            r#type: r#type.as_ref().to_string(),
            status: data::OpStatus::Pending.as_ref().to_string(),
            event,
            options,
            create_time: now,
            update_time: now,
            v: data::Op::version(),
        };
        collection.create(&op).await?;
        Ok(())
    }

    /// Load every outbox record that was not durably completed — the crash
    /// replay set. Order is stable across restarts (creation time).
    pub async fn load_pending_ops(&self) -> Result<Vec<data::Op>> {
        let q = Query::new().filter(Filter::and().expr(Expr::r#in(
            "status",
            vec![
                data::OpStatus::Pending.as_ref(),
                data::OpStatus::Dispatched.as_ref(),
                data::OpStatus::Overflow.as_ref(),
            ],
        )));
        Ok(self.ops().query(&q).await?.rows)
    }

    /// Mark a record as handed to the in-memory scheduler. Boot recovery still
    /// treats this state as replayable; periodic overflow recovery does not.
    pub async fn mark_op_dispatched(&self, pid: &str, tid: &str, r#type: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        for mut op in collection.query(&q).await?.rows {
            if op.r#type == r#type
                && (op.status == data::OpStatus::Pending.as_ref()
                    || op.status == data::OpStatus::Overflow.as_ref())
            {
                op.status = data::OpStatus::Dispatched.as_ref().to_string();
                op.update_time = utils::time::time_millis();
                collection.update(&op).await?;
            }
        }
        Ok(())
    }

    /// Load stale, not-yet-dispatched overflow records. `Dispatched` records
    /// are deliberately excluded while the engine is running: replaying them
    /// would duplicate work that is queued or executing in memory.
    pub async fn load_overflow_ops(&self, older_than_millis: i64) -> Result<Vec<data::Op>> {
        let q = Query::new()
            .filter(Filter::and().expr(Expr::r#in(
                "status",
                vec![
                    data::OpStatus::Pending.as_ref(),
                    data::OpStatus::Overflow.as_ref(),
                ],
            )))
            .order("create_time", Sort::Asc);
        let now = utils::time::time_millis();
        Ok(self
            .ops()
            .query(&q)
            .await?
            .rows
            .into_iter()
            .filter(|op| now - op.create_time >= older_than_millis)
            .collect())
    }

    /// Close the in-flight outbox records of a task (`Pending`/`Dispatched`/
    /// `Overflow` → `Done`),
    /// filtered by operation type: a `next` close must not sweep away a
    /// concurrent client-action record of the same task (and vice versa). Must
    /// only be called after the operation's effects (the task state write,
    /// including the `NEXT_COMPLETE` marker) were durably persisted — the
    /// writer FIFO order guarantees this.
    pub async fn complete_ops(&self, pid: &str, tid: &str, r#type: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        for mut op in collection.query(&q).await?.rows {
            if op.r#type == r#type
                && (op.status == data::OpStatus::Pending.as_ref()
                    || op.status == data::OpStatus::Dispatched.as_ref()
                    || op.status == data::OpStatus::Overflow.as_ref())
            {
                op.status = data::OpStatus::Done.as_ref().to_string();
                op.update_time = utils::time::time_millis();
                collection.update(&op).await?;
            }
        }
        Ok(())
    }

    /// Mark a pending `next` record as overflowed to the durable scheduler disk
    /// queue after the bounded in-memory queue rejected it.
    pub async fn mark_op_overflow(&self, pid: &str, tid: &str, r#type: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        for mut op in collection.query(&q).await?.rows {
            if op.r#type == r#type && op.status == data::OpStatus::Pending.as_ref() {
                op.status = data::OpStatus::Overflow.as_ref().to_string();
                op.update_time = utils::time::time_millis();
                collection.update(&op).await?;
            }
        }
        Ok(())
    }

    /// Drop every outbox record of a process (used when the process is removed).
    pub async fn remove_ops(&self, pid: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
        for op in collection.query(&q).await?.rows {
            collection.delete(&op.id).await?;
        }
        Ok(())
    }

    /// Advance a stored delivery from `Created` to `Delivered` — the channel
    /// handler ran to completion, so the delivery succeeded. Only rows still
    /// `Created` move: a handler that acked (or was closed) while running
    /// must never be downgraded.
    pub async fn mark_delivered(&self, id: &str) -> Result<()> {
        if let Ok(mut delivery) = self.deliveries().find(id).await
            && delivery.status == DeliveryStatus::Created
        {
            delivery.status = DeliveryStatus::Delivered;
            delivery.update_time = utils::time::time_millis();
            self.deliveries().update(&delivery).await?;
        }
        Ok(())
    }

    /// Ack one delivery row (by its delivery id): set its status.
    pub async fn set_delivery(&self, id: &str, status: DeliveryStatus) -> Result<()> {
        if let Ok(mut delivery) = self.deliveries().find(id).await {
            // `Completed` is the final state (the engine closed the
            // delivery) — a late ack must never downgrade it back to the
            // intermediate `Acked`
            if delivery.status == DeliveryStatus::Completed {
                return Ok(());
            }
            let pid = delivery.pid.clone();
            delivery.status = status;
            delivery.update_time = utils::time::time_millis();
            self.deliveries().update(&delivery).await?;
            // a delivery closed `Completed` by the engine may be the
            // process's last unsettled one — if the process is finished and
            // nothing is left unsettled, mark it removable for the sweeper.
            // `Acked` is only an intermediate state and never triggers the
            // mark. `Error` keeps the process alive for manual handling.
            if status == DeliveryStatus::Completed {
                let _ = self.try_mark_removable(&pid).await;
            }
        }

        // it's ok there is no delivery
        Ok(())
    }

    /// Mark every delivery row of a task (pid, tid) with a status — used to
    /// close the deliveries when the task completes.
    pub async fn set_deliveries_with(
        &self,
        pid: &str,
        tid: &str,
        status: DeliveryStatus,
    ) -> Result<bool> {
        debug!("set_deliveries_with pid={pid} tid={tid} status={status:?}");
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        let collection = self.deliveries();
        if let Ok(deliveries) = collection.query(&q).await {
            for m in deliveries.rows.iter() {
                let mut m = m.clone();
                m.status = status;
                m.update_time = utils::time::time_millis();
                collection.update(&m).await?;
            }
        }

        // it's ok there is no delivery
        // whether a delivery exists depends on the emitter
        // it is allowed the client creates emitter without emit_id
        Ok(true)
    }

    /// Collect deliveries with no response: re-arm the ones that were handed
    /// over but never acked (`Delivered` — as well as `Created` rows that were
    /// never successfully dispatched) and mark the ones that exceeded
    /// `max_delivery_retry_times` as errors. Returns every re-armed delivery
    /// (the caller re-sends them to their own channels).
    pub async fn with_no_response_deliveries(
        &self,
        timeout_millis: i64,
        max_delivery_retry_times: i32,
    ) -> Result<Vec<data::Delivery>> {
        let q = Query::new().limit(300).filter(Filter::and().expr(Expr::lt(
            "update_time",
            utils::time::time_millis() - timeout_millis,
        )));
        let collection = self.deliveries();
        let mut rearmed = Vec::new();
        if let Ok(deliveries) = collection.query(&q).await {
            for m in deliveries.rows.iter() {
                // only rows that still need a response: never successfully
                // dispatched (`Created`) or handed over but not acked/closed
                // (`Delivered`); settled ones are skipped
                if !matches!(
                    m.status,
                    DeliveryStatus::Created | DeliveryStatus::Delivered
                ) {
                    continue;
                }
                let mut delivery = m.clone();
                delivery.update_time = utils::time::time_millis();
                if delivery.retry_times < max_delivery_retry_times {
                    delivery.retry_times += 1;
                    if collection.update(&delivery).await? {
                        rearmed.push(delivery);
                    }
                } else {
                    // the delivery will re-send by manual through the manager
                    // command — an errored delivery keeps its process alive
                    // until a manual resend/clear resolves it
                    delivery.status = DeliveryStatus::Error;
                    collection.update(&delivery).await?;
                }
            }
        }
        Ok(rearmed)
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub async fn resend_error_deliveries(&self) -> Result<()> {
        let collection = self.deliveries();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("status", DeliveryStatus::Error)));
        if let Ok(deliveries) = collection.query(&q).await {
            for m in deliveries.rows.iter() {
                let mut delivery = m.clone();
                delivery.status = DeliveryStatus::Created;
                delivery.retry_times = 0;
                delivery.update_time = utils::time::time_millis();
                collection.update(&delivery).await?;
            }
        }

        Ok(())
    }

    /// Delete error delivery rows: all of them or only those of one process.
    pub async fn clear_error_deliveries(&self, pid: Option<String>) -> Result<()> {
        let collection = self.deliveries();
        let mut cond = Filter::and().expr(Expr::eq("status", DeliveryStatus::Error));
        if let Some(pid) = &pid {
            cond = cond.expr(Expr::eq("pid", pid));
        }

        let q = Query::new().filter(cond);
        if let Ok(deliveries) = collection.query(&q).await {
            for m in deliveries.rows.iter() {
                collection.delete(&m.id).await?;
            }
        }

        Ok(())
    }

    /// Reset one error delivery row back to `Created` for redelivery. Returns
    /// the delivery when it was an error delivery and was reset, `None`
    /// otherwise.
    pub async fn resend_error_delivery(&self, delivery_id: &str) -> Result<Option<data::Delivery>> {
        let collection = self.deliveries();
        let mut delivery = match collection.find(delivery_id).await {
            Ok(delivery) => delivery,
            Err(_) => return Ok(None),
        };
        if delivery.status != DeliveryStatus::Error {
            return Ok(None);
        }

        delivery.status = DeliveryStatus::Created;
        delivery.retry_times = 0;
        delivery.update_time = utils::time::time_millis();
        if collection.update(&delivery).await? {
            Ok(Some(delivery))
        } else {
            Ok(None)
        }
    }

    /// Delete one error delivery row. Returns `true` when the row existed and
    /// was in error state and was deleted.
    pub async fn clear_error_delivery(&self, delivery_id: &str) -> Result<bool> {
        let collection = self.deliveries();
        match collection.find(delivery_id).await {
            Ok(delivery) if delivery.status == DeliveryStatus::Error => {
                collection.delete(delivery_id).await
            }
            _ => Ok(false),
        }
    }

    pub async fn upsert_task(&self, task: &Arc<scheduler::Task>) -> Result<()> {
        debug!(pid = %task.pid, tid = %task.id, "upsert task");
        let data: data::Task = task.into_data()?;
        self.upsert_task_data(&data).await
    }
    pub async fn upsert_task_data(&self, data: &data::Task) -> Result<()> {
        let collection = self.tasks();
        match collection.find(&data.id).await {
            Ok(_) => {
                collection.update(data).await?;
            }
            Err(_) => {
                collection.create(data).await?;
            }
        }

        Ok(())
    }

    /// Persist one task scope's vars row (data + sealed). Called by the
    /// persist path only for scopes whose vars actually changed.
    pub async fn upsert_task_vars(&self, task: &Arc<scheduler::Task>) -> Result<()> {
        debug!(pid = %task.pid, tid = %task.id, "upsert task vars");
        let data: data::TaskVars = task.into_data_vars()?;
        let collection = self.vars();
        match collection.find(&data.id).await {
            Ok(_) => {
                collection.update(&data).await?;
            }
            Err(_) => {
                collection.create(&data).await?;
            }
        }

        Ok(())
    }

    /// Durable write of a task lifecycle row and every scope vars row that
    /// diverged: the task's own row, then — walking the parent chain to the
    /// root — each ancestor whose vars changed since its last flush (the
    /// scope that owns an updated key, which `update_data` resolved at write
    /// time). A lifecycle-only transition (state/timing change, no data
    /// touched) writes just the one lifecycle row; scope vars rows are
    /// written exactly when the owning scope actually mutated. The dirty
    /// flags are cleared only after each row is durable, so a crash between
    /// mutations and the next persist loses nothing that the previous design
    /// would have kept.
    pub async fn persist_task_rows(&self, task: &Arc<scheduler::Task>) -> Result<()> {
        self.upsert_task(task).await?;
        let mut scope = Some(task.clone());
        while let Some(t) = scope {
            if t.is_vars_dirty() {
                // the vars row must capture every mutation that happened
                // before the serialization; the generation read here is
                // compared again after the durable write, and the dirty flag
                // is cleared only when no mutation raced it — a mutation that
                // landed while the row was being written keeps the scope
                // dirty so the next persist persists it (clearing it away
                // would durably lose the mutation, e.g. a `NEXT_COMPLETE`
                // marker that recovery relies on)
                let generation = t.vars_gen();
                self.upsert_task_vars(&t).await?;
                if t.vars_gen() == generation {
                    t.clear_vars_dirty();
                }
            }
            scope = t.parent();
        }
        Ok(())
    }

    pub async fn mark_proc_complete(
        &self,
        pid: &str,
        end_time: i64,
        state: TaskState,
    ) -> Result<()> {
        let collection = self.procs();
        let mut proc = collection.find(pid).await?;
        proc.end_time = end_time;
        proc.state = state.into();
        collection.update(&proc).await?;
        Ok(())
    }

    pub async fn upsert_proc(&self, proc: &Arc<scheduler::Process>) -> Result<()> {
        debug!("upsert process: {}", proc.id());
        let collection = self.procs();
        let data: data::Proc = proc.into_data()?;
        match collection.find(proc.id()).await {
            Ok(_) => {
                collection.update(&data).await?;
            }
            Err(_) => {
                collection.create(&data).await?;
            }
        }

        Ok(())
    }

    async fn load_tasks(&self, proc: &Arc<scheduler::Process>, rt: &Arc<Runtime>) -> Result<()> {
        debug!("load_tasks pid={}", proc.id());
        let collection = self.tasks();
        let query = Query::new().filter(Filter::and().expr(Expr::eq("pid", proc.id())));
        let tasks = collection.query(&query).await?;

        // phase 1 + 2: load tasks and register dynamic nodes into the tree
        // map so node links (parent/prev/next) can be resolved afterwards,
        // then rebuild the dynamic node graph. The tree guard is scoped to
        // these synchronous phases — the vars attach below awaits.
        {
            let tree = &proc.tree();
            let mut dyn_nodes: Vec<(Arc<Node>, NodeData)> = Vec::new();
            for t in tasks.rows {
                let data: NodeData = serde_json::from_str(&t.node_data)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                let node = match tree.node(&data.id) {
                    Some(node) => node,
                    None => {
                        let node = tree.get_or_make(&data.id, data.content.clone(), data.level)?;
                        dyn_nodes.push((node.clone(), data));
                        node
                    }
                };

                let state: TaskState = t.state.into();
                let mut task = scheduler::Task::new(proc, &t.tid, node, rt);
                task.set_pure_state(state.clone());
                task.set_start_time(t.start_time);
                task.set_end_time(t.end_time);
                task.timestamp = t.timestamp;
                if let Some(prev) = &t.prev {
                    task.set_prev(prev);
                }

                if let Some(parent) = &t.parent {
                    task.set_parent(parent);
                }

                // resume next tasks
                for next in t.next.iter() {
                    task.set_next(next);
                }

                if let Some(err) = t.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    task.set_pure_err(&err)
                }
                proc.push_task(Arc::new(task))?;
            }

            for (node, data) in dyn_nodes.iter() {
                node.restore_links(data, tree);
            }
            dyn_nodes
        };

        // phase 3: attach each task scope's persisted vars (its own data and
        // sealed rows) onto the restored tasks — scope vars live in the vars
        // collection, keyed by the same composite id as the lifecycle row
        let vars = self.vars();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", proc.id())));
        for row in vars.query(&q).await?.rows {
            let Some(task) = proc.task(&row.tid) else {
                continue;
            };
            if !row.data.is_empty() {
                let data = serde_json::from_str(&row.data)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_data(&data);
            }
            if !row.sealed.is_empty() {
                let data = serde_json::from_str(&row.sealed)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_sealed_data(&data);
            }
        }

        Ok(())
    }
}
