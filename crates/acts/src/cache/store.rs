use crate::{
    ActError, Error, Result, Vars, Workflow,
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
        let Some(p) = self.procs().find_opt(pid).await? else {
            return Ok(None);
        };
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
            let err: Error =
                serde_json::from_str(&err).map_err(|err| ActError::Store(err.to_string()))?;
            proc.set_pure_err(&err)
        }
        Ok(Some(proc))
    }

    pub async fn remove_proc(&self, pid: &str) -> Result<bool> {
        debug!("remove_proc pid={}", pid);
        // All rows of the process — tasks, outbox ops, its message/delivery
        // rows and the proc row — are removed as ONE atomic batch, so a crash
        // mid-removal cannot leave a half-deleted process behind nor orphaned
        // message/delivery rows that would be retried forever.
        self.remove_proc_rows(pid).await
    }

    /// The directory a process's filesystem access was confined to, read from
    /// its durable row: the process env carries it under a private key, so it
    /// is still reachable once the in-memory instance is gone — the sweeper
    /// removes a *finished* process's directory, and a start that never became
    /// durable removes its own. `None` when there is no row, the process was
    /// started without an ACL workdir, or the stored env cannot be read: the
    /// directory is reclaimed best-effort and a row that cannot answer must
    /// never be a reason to keep the process's rows alive.
    pub(crate) async fn proc_workdir(&self, pid: &str) -> Option<std::path::PathBuf> {
        let proc = self.procs().find_opt(pid).await.ok()??;
        let env: serde_json::Value = serde_json::from_str(&proc.env).ok()?;
        Vars::from(env).get::<std::path::PathBuf>(utils::consts::PROC_WORKDIR)
    }

    /// but not yet run. Deduplicated per `(pid, tid, type)` — at most one
    /// in-flight record per operation, matching the previous
    /// `Sign::NEXT_PENDING` semantics. Queued on the store writer (FIFO)
    /// *before* the in-memory queue dispatch, after the task state write, so a
    /// `Pending` record always has a durable task behind it.
    pub async fn enqueue_next_op(&self, pid: &str, tid: &str) -> Result<()> {
        self.enqueue_next_op_details(pid, tid, None, 0).await
    }

    pub async fn enqueue_next_op_details(
        &self,
        pid: &str,
        tid: &str,
        target_tid: Option<String>,
        source_version: i64,
    ) -> Result<()> {
        self.enqueue_op(
            pid,
            tid,
            tid,
            data::OpType::Next,
            None,
            None,
            target_tid,
            source_version,
        )
        .await
    }

    /// Record a durable outbox entry for task execution. This is the disk
    /// overflow queue used when the in-memory scheduler queue is full.
    pub async fn enqueue_exec_op(&self, pid: &str, tid: &str) -> Result<()> {
        self.enqueue_op(pid, tid, tid, data::OpType::Exec, None, None, None, 0)
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
            tid,
            data::OpType::Action,
            Some(event.to_string()),
            Some(options.to_string()),
            None,
            0,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn enqueue_op(
        &self,
        pid: &str,
        tid: &str,
        source_tid: &str,
        r#type: data::OpType,
        event: Option<String>,
        options: Option<String>,
        target_tid: Option<String>,
        source_version: i64,
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
        let existing = collection.query_all(&q).await?;
        if existing
            .iter()
            .any(|op| op.r#type == r#type.as_ref() && op.status == data::OpStatus::Pending.as_ref())
        {
            return Ok(());
        }

        let now = utils::time::time_millis();
        let op = data::Op {
            id: utils::longid(),
            pid: pid.to_string(),
            source_tid: source_tid.to_string(),
            tid: tid.to_string(),
            target_tid,
            source_version,
            phase: data::OpPhase::DurablePending.as_ref().to_string(),
            causal_op_id: None,
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
    /// replay set. Read exhaustively — the engine replays exactly what this
    /// returns, so a page limit must never drop a record (id order, which is
    /// stable across restarts).
    pub async fn load_pending_ops(&self) -> Result<Vec<data::Op>> {
        let q = Query::new().filter(Filter::and().expr(Expr::r#in(
            "status",
            vec![
                data::OpStatus::Pending.as_ref(),
                data::OpStatus::Dispatched.as_ref(),
                data::OpStatus::Overflow.as_ref(),
            ],
        )));
        self.ops().query_all(&q).await
    }

    /// Whether a task still has an unresolved Error/Abort propagation hop.
    /// Terminal parent completion must wait for these operations: otherwise a
    /// normal completion pass can run between the source failure and the queued
    /// ancestor handler and mark the process completed before the failure does.
    pub async fn has_pending_propagation(&self, pid: &str, tid: &str) -> Result<bool> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        Ok(collection.query_all(&q).await?.iter().any(|op| {
            matches!(op.r#type.as_str(), "error" | "abort")
                && op.status != data::OpStatus::Done.as_ref()
        }))
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
        for mut op in collection.query_all(&q).await? {
            if op.r#type == r#type
                && (op.status == data::OpStatus::Pending.as_ref()
                    || op.status == data::OpStatus::Overflow.as_ref())
            {
                op.status = data::OpStatus::Dispatched.as_ref().to_string();
                op.phase = data::OpPhase::Dispatched.as_ref().to_string();
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
            .query_all(&q)
            .await?
            .into_iter()
            .filter(|op| now - op.create_time >= older_than_millis)
            .collect())
    }

    /// Close the in-flight outbox records of a task (`Pending`/`Dispatched`/
    /// `Overflow` → `Done`),
    /// filtered by operation type: a `next` close must not sweep away a
    /// concurrent client-action record of the same task (and vice versa). Must
    /// only be called after the operation's effects (the task state write,
    /// including the applied propagation phase) were durably persisted — the
    /// writer FIFO order guarantees this.
    pub async fn complete_ops(&self, pid: &str, tid: &str, r#type: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        for mut op in collection.query_all(&q).await? {
            if op.r#type == r#type
                && (op.status == data::OpStatus::Pending.as_ref()
                    || op.status == data::OpStatus::Dispatched.as_ref()
                    || op.status == data::OpStatus::Overflow.as_ref())
            {
                op.status = data::OpStatus::Done.as_ref().to_string();
                op.phase = data::OpPhase::Completed.as_ref().to_string();
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
        for mut op in collection.query_all(&q).await? {
            if op.r#type == r#type && op.status == data::OpStatus::Pending.as_ref() {
                op.status = data::OpStatus::Overflow.as_ref().to_string();
                // Overflow changes where the operation waits, not the caller
                // phase: it remains Durable-Pending.
                op.update_time = utils::time::time_millis();
                collection.update(&op).await?;
            }
        }
        Ok(())
    }

    /// Drop every outbox record of a process (used when the process is removed).
    pub async fn remove_ops(&self, pid: &str) -> Result<()> {
        // Exhaustive: the ids come from the complete match set, so no outbox
        // row (or index row) of the process is left behind as an orphan.
        self.ops()
            .delete_all(Some(&Filter::and().expr(Expr::eq("pid", pid.to_string()))))
            .await
    }

    /// Advance the lifecycle phase of in-flight records. The update is
    /// forward-only and filtered by operation type, so a phase note for a
    /// `next` cannot close or regress a concurrent action/error/abort record.
    pub async fn mark_op_phase(
        &self,
        pid: &str,
        tid: &str,
        r#type: &str,
        next: data::OpPhase,
    ) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        for mut op in collection.query_all(&q).await? {
            let current = data::OpPhase::from_task_value(op.phase.as_str());
            if op.r#type == r#type
                && op.status != data::OpStatus::Done.as_ref()
                && current.can_advance_to(next)
            {
                op.phase = next.as_ref().to_string();
                op.update_time = utils::time::time_millis();
                collection.update(&op).await?;
            }
        }
        Ok(())
    }

    /// Record a durable one-hop error/abort propagation. `tid` is the target
    /// whose propagation job will run; `source_tid` names the task whose
    /// terminal outcome caused it.
    pub async fn enqueue_propagation_op(
        &self,
        source_tid: &str,
        source_version: i64,
        target: &crate::scheduler::Task,
        r#type: data::OpType,
    ) -> Result<()> {
        let target_tid = target.parent_id();
        self.enqueue_op(
            &target.pid,
            &target.id,
            source_tid,
            r#type,
            None,
            None,
            target_tid,
            source_version,
        )
        .await
    }

    /// Advance a stored delivery from `Created` to `Delivered` — the channel
    /// handler ran to completion, so the delivery succeeded. Only rows still
    /// `Created` move: a handler that acked (or was closed) while running
    /// must never be downgraded. The stored row is read under its document
    /// lock (`Store::update_delivery`), so a close or retry-pass write landing
    /// while the handler ran is seen — never overwritten by a stale `Created`.
    pub async fn mark_delivered(&self, id: &str) -> Result<()> {
        self.update_delivery(id, |mut delivery| {
            (delivery.status == DeliveryStatus::Created).then(|| {
                delivery.status = DeliveryStatus::Delivered;
                delivery
            })
        })
        .await?;
        Ok(())
    }

    /// Ack one delivery row (by its delivery id): set its status.
    pub async fn set_delivery(&self, id: &str, status: DeliveryStatus) -> Result<()> {
        let delivery = self
            .update_delivery(id, |mut delivery| {
                // `Completed` is the final state (the engine closed the
                // delivery) — a late ack must never downgrade it back to the
                // intermediate `Acked`. The stored status is read under the
                // row's lock, so an engine close that won the race is seen
                // here instead of being overwritten by a stale read.
                (delivery.status != DeliveryStatus::Completed).then(|| {
                    delivery.status = status;
                    delivery
                })
            })
            .await?;
        // it's ok there is no delivery, or it was already closed by the engine
        let Some(delivery) = delivery else {
            return Ok(());
        };
        // a delivery closed `Completed` by the engine may be the
        // process's last unsettled one — if the process is finished and
        // nothing is left unsettled, mark it removable for the sweeper.
        // `Acked` is only an intermediate state and never triggers the
        // mark. `Error` keeps the process alive for manual handling.
        if status == DeliveryStatus::Completed {
            let _ = self.try_mark_removable(&delivery.pid).await;
        }
        Ok(())
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub async fn resend_error_deliveries(&self) -> Result<()> {
        let collection = self.deliveries();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("status", DeliveryStatus::Error)));
        for mut delivery in collection.query_all(&q).await? {
            delivery.status = DeliveryStatus::Created;
            delivery.retry_times = 0;
            delivery.update_time = utils::time::time_millis();
            collection.update(&delivery).await?;
        }

        Ok(())
    }

    /// Delete error delivery rows: all of them or only those of one process.
    pub async fn clear_error_deliveries(&self, pid: Option<String>) -> Result<()> {
        let mut cond = Filter::and().expr(Expr::eq("status", DeliveryStatus::Error));
        if let Some(pid) = &pid {
            cond = cond.expr(Expr::eq("pid", pid));
        }

        // Exhaustive: an error row past a page limit must not survive as an
        // orphan that nothing ever retries or clears.
        self.deliveries().delete_all(Some(&cond)).await?;

        Ok(())
    }

    /// Reset one error delivery row back to `Created` for redelivery. Returns
    /// the delivery when it was an error delivery and was reset, `None`
    /// otherwise.
    pub async fn resend_error_delivery(&self, delivery_id: &str) -> Result<Option<data::Delivery>> {
        let collection = self.deliveries();
        let Some(mut delivery) = collection.find_opt(delivery_id).await? else {
            return Ok(None);
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
        let Some(delivery) = collection.find_opt(delivery_id).await? else {
            return Ok(false);
        };
        if delivery.status == DeliveryStatus::Error {
            collection.delete(delivery_id).await
        } else {
            Ok(false)
        }
    }

    pub async fn upsert_task(&self, task: &Arc<scheduler::Task>) -> Result<()> {
        debug!(pid = %task.pid, tid = %task.id, "upsert task");
        let data: data::Task = task.into_data()?;
        self.upsert_task_data(&data).await
    }
    pub async fn upsert_task_data(&self, data: &data::Task) -> Result<()> {
        let collection = self.tasks();
        if collection.find_opt(&data.id).await?.is_some() {
            collection.update(data).await?;
        } else {
            collection.create(data).await?;
        }

        Ok(())
    }

    /// Persist one task scope's vars row (data + sealed). Called by the
    /// persist path only for scopes whose vars actually changed.
    pub async fn upsert_task_vars(&self, task: &Arc<scheduler::Task>) -> Result<()> {
        debug!(pid = %task.pid, tid = %task.id, "upsert task vars");
        let data: data::TaskVars = task.into_data_vars()?;
        let collection = self.vars();
        if collection.find_opt(&data.id).await?.is_some() {
            collection.update(&data).await?;
        } else {
            collection.create(&data).await?;
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
                // would durably lose the mutation, e.g. an applied propagation
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
        if collection.find_opt(proc.id()).await?.is_some() {
            collection.update(&data).await?;
        } else {
            collection.create(&data).await?;
        }

        Ok(())
    }

    async fn load_tasks(&self, proc: &Arc<scheduler::Process>, rt: &Arc<Runtime>) -> Result<()> {
        debug!("load_tasks pid={}", proc.id());
        let collection = self.tasks();
        let query = Query::new().filter(Filter::and().expr(Expr::eq("pid", proc.id())));
        let tasks = collection.query_all(&query).await?;

        // phase 1 + 2: load tasks and register dynamic nodes into the tree
        // map so node links (parent/prev/next) can be resolved afterwards,
        // then rebuild the dynamic node graph. The tree guard is scoped to
        // these synchronous phases — the vars attach below awaits.
        {
            let tree = &proc.tree();
            let mut dyn_nodes: Vec<(Arc<Node>, NodeData)> = Vec::new();
            for t in tasks {
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
        for row in vars.query_all(&q).await? {
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
