use crate::{
    ActError, Error, Result, Workflow,
    data::{self, MessageStatus},
    scheduler::{self, Node, NodeData, Runtime, TaskState},
    store::{DbCollectionIden, Store, query::*},
    utils,
};
use std::{collections::HashSet, sync::Arc};
use tracing::debug;

impl Store {
    pub fn load(
        &self,
        cap: usize,
        rt: &Arc<Runtime>,
        skip: &HashSet<String>,
    ) -> Result<Vec<Arc<scheduler::Process>>> {
        debug!("load cap={}", cap);
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new().filter(
                Filter::or()
                    .expr(Expr::eq("state", TaskState::None.to_string()))
                    .expr(Expr::eq("state", TaskState::Ready.to_string()))
                    .expr(Expr::eq("state", TaskState::Running.to_string()))
                    .expr(Expr::eq("state", TaskState::Pending.to_string())),
            );
            let procs = self.procs().query(&query)?;
            for p in procs.rows {
                if skip.contains(&p.id) {
                    continue;
                }
                let model = Workflow::from_json(&p.model)?;
                let env_local: serde_json::Value =
                    serde_json::from_str(&p.env).map_err(|err| ActError::Store(err.to_string()))?;
                let state = p.state.clone();
                let proc = scheduler::Process::new_with_timestamp(&p.id, p.timestamp, rt);

                proc.load(&model)?;
                proc.set_pure_state(state.into());
                proc.set_start_time(p.start_time);
                proc.set_end_time(p.end_time);
                proc.set_env(&env_local.into());
                if let Some(err) = p.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    proc.set_pure_err(&err)
                }

                self.load_tasks(&proc, rt)?;
                ret.push(proc);
                if ret.len() >= cap {
                    break;
                }
            }
        }

        Ok(ret)
    }

    pub fn load_proc(
        &self,
        pid: &str,
        rt: &Arc<Runtime>,
    ) -> Result<Option<Arc<scheduler::Process>>> {
        debug!("load process pid={}", pid);
        match self.procs().find(pid) {
            Ok(p) => {
                // println!("process model={}", p.model);
                let model = Workflow::from_json(&p.model)?;
                let proc = scheduler::Process::new(pid, rt);
                let env_local: serde_json::Value =
                    serde_json::from_str(&p.env).map_err(|err| ActError::Store(err.to_string()))?;

                proc.load(&model)?;
                proc.set_pure_state(p.state.into());
                proc.set_start_time(p.start_time);
                proc.set_env(&env_local.into());
                self.load_tasks(&proc, rt)?;
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

    pub fn remove_proc(&self, pid: &str) -> Result<bool> {
        debug!("remove_proc pid={}", pid);
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
        let tasks = self.tasks().query(&q)?;
        for task in tasks.rows {
            self.tasks().delete(&task.id)?;
        }
        self.remove_ops(pid)?;
        self.procs().delete(pid)?;
        Ok(true)
    }

    /// but not yet run. Deduplicated per `(pid, tid, type)` — at most one
    /// in-flight record per operation, matching the previous
    /// `Sign::NEXT_PENDING` semantics. Queued on the store writer (FIFO)
    /// *before* the in-memory queue dispatch, after the task state write, so a
    /// `Pending` record always has a durable task behind it.
    pub fn enqueue_next_op(&self, pid: &str, tid: &str) -> Result<()> {
        self.enqueue_op(pid, tid, data::OpType::Next, None, None)
    }

    /// Record a durable outbox entry for a client action (event + options).
    /// Deduplicated per `(pid, tid, type)`, so it is not shadowed by the
    /// task's in-flight `next` record (an interrupt act keeps its `next` op
    /// `Pending` while waiting for the client). Written before the action is
    /// applied so recovery can re-apply it when the crash happened before the
    /// task state write became durable.
    pub fn enqueue_action_op(
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
    }

    fn enqueue_op(
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
        let existing = collection.query(&q)?;
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
        collection.create(&op)?;
        Ok(())
    }

    /// Load every outbox record that was not durably completed — the crash
    /// replay set. Order is stable across restarts (creation time).
    pub fn load_pending_ops(&self) -> Result<Vec<data::Op>> {
        let q = Query::new()
            .filter(Filter::and().expr(Expr::eq("status", data::OpStatus::Pending.as_ref())));
        Ok(self.ops().query(&q)?.rows)
    }

    /// Close the in-flight outbox records of a task (`Pending` → `Done`),
    /// filtered by operation type: a `next` close must not sweep away a
    /// concurrent client-action record of the same task (and vice versa). Must
    /// only be called after the operation's effects (the task state write,
    /// including the `NEXT_COMPLETE` marker) were durably persisted — the
    /// writer FIFO order guarantees this.
    pub fn complete_ops(&self, pid: &str, tid: &str, r#type: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        for mut op in collection.query(&q)?.rows {
            if op.r#type == r#type && op.status == data::OpStatus::Pending.as_ref() {
                op.status = data::OpStatus::Done.as_ref().to_string();
                op.update_time = utils::time::time_millis();
                collection.update(&op)?;
            }
        }
        Ok(())
    }

    /// Drop every outbox record of a process (used when the process is removed).
    pub fn remove_ops(&self, pid: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
        for op in collection.query(&q)?.rows {
            collection.delete(&op.id)?;
        }
        Ok(())
    }

    /// Ack one delivery row (by its delivery id): set its status.
    pub fn set_delivery(&self, id: &str, status: MessageStatus) -> Result<()> {
        if let Ok(mut delivery) = self.deliveries().find(id) {
            delivery.status = status;
            delivery.update_time = utils::time::time_millis();
            self.deliveries().update(&delivery)?;
        }

        // it's ok there is no delivery
        Ok(())
    }

    /// Mark every delivery row of a task (pid, tid) with a status — used to
    /// close the deliveries when the task completes.
    pub fn set_deliveries_with(&self, pid: &str, tid: &str, status: MessageStatus) -> Result<bool> {
        debug!("set_deliveries_with pid={pid} tid={tid} status={status:?}");
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        let collection = self.deliveries();
        if let Ok(deliveries) = collection.query(&q) {
            for m in deliveries.rows.iter() {
                let mut m = m.clone();
                m.status = status;
                m.update_time = utils::time::time_millis();
                collection.update(&m)?;
            }
        }

        // it's ok there is no delivery
        // whether a delivery exists depends on the emitter
        // it is allowed the client creates emitter without emit_id
        Ok(true)
    }

    /// Collect deliveries with no response: re-send the not-yet-acked ones and
    /// mark the ones that exceeded `max_delivery_retry_times` as errors. The
    /// callback receives each delivery that was re-armed.
    pub fn with_no_response_deliveries<F: Fn(&data::Delivery)>(
        &self,
        timeout_millis: i64,
        max_delivery_retry_times: i32,
        f: F,
    ) -> Result<()> {
        let q = Query::new().limit(300).filter(
            Filter::and()
                .expr(Expr::eq("status", MessageStatus::Created))
                .expr(Expr::lt(
                    "update_time",
                    utils::time::time_millis() - timeout_millis,
                )),
        );
        let collection = self.deliveries();
        if let Ok(deliveries) = collection.query(&q) {
            for m in deliveries.rows.iter() {
                let mut delivery = m.clone();
                delivery.update_time = utils::time::time_millis();
                if delivery.retry_times < max_delivery_retry_times {
                    delivery.retry_times += 1;
                    if collection.update(&delivery)? {
                        f(&delivery);
                    }
                } else {
                    // the delivery will re-send by manual through the manager command
                    delivery.status = MessageStatus::Error;
                    collection.update(&delivery)?;
                }
            }
        }
        Ok(())
    }

    /// Re-send every error delivery row (reset to `Created`; the retry timer
    /// sends them to their own channels).
    pub fn resend_error_deliveries(&self) -> Result<()> {
        let collection = self.deliveries();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("status", MessageStatus::Error)));
        if let Ok(deliveries) = collection.query(&q) {
            for m in deliveries.rows.iter() {
                let mut delivery = m.clone();
                delivery.status = MessageStatus::Created;
                delivery.retry_times = 0;
                delivery.update_time = utils::time::time_millis();
                collection.update(&delivery)?;
            }
        }

        Ok(())
    }

    /// Delete error delivery rows: all of them or only those of one process.
    pub fn clear_error_deliveries(&self, pid: Option<String>) -> Result<()> {
        let collection = self.deliveries();
        let mut cond = Filter::and().expr(Expr::eq("status", MessageStatus::Error));
        if let Some(pid) = &pid {
            cond = cond.expr(Expr::eq("pid", pid));
        }

        let q = Query::new().filter(cond);
        if let Ok(deliveries) = collection.query(&q) {
            for m in deliveries.rows.iter() {
                collection.delete(&m.id)?;
            }
        }

        Ok(())
    }

    /// Reset one error delivery row back to `Created` for redelivery. Returns
    /// the delivery when it was an error delivery and was reset, `None`
    /// otherwise.
    pub fn resend_error_delivery(&self, delivery_id: &str) -> Result<Option<data::Delivery>> {
        let collection = self.deliveries();
        let mut delivery = match collection.find(delivery_id) {
            Ok(delivery) => delivery,
            Err(_) => return Ok(None),
        };
        if delivery.status != MessageStatus::Error {
            return Ok(None);
        }

        delivery.status = MessageStatus::Created;
        delivery.retry_times = 0;
        delivery.update_time = utils::time::time_millis();
        if collection.update(&delivery)? {
            Ok(Some(delivery))
        } else {
            Ok(None)
        }
    }

    /// Delete one error delivery row. Returns `true` when the row existed and
    /// was in error state and was deleted.
    pub fn clear_error_delivery(&self, delivery_id: &str) -> Result<bool> {
        let collection = self.deliveries();
        match collection.find(delivery_id) {
            Ok(delivery) if delivery.status == MessageStatus::Error => {
                collection.delete(delivery_id)
            }
            _ => Ok(false),
        }
    }

    pub fn upsert_task(&self, task: &Arc<scheduler::Task>) -> Result<()> {
        debug!(pid = %task.pid, tid = %task.id, "upsert task");
        let data: data::Task = task.into_data()?;
        self.upsert_task_data(&data)
    }

    pub fn upsert_task_data(&self, data: &data::Task) -> Result<()> {
        let collection = self.tasks();
        match collection.find(&data.id) {
            Ok(_) => {
                collection.update(data)?;
            }
            Err(_) => {
                collection.create(data)?;
            }
        }

        Ok(())
    }

    pub fn mark_proc_complete(&self, pid: &str, end_time: i64, state: TaskState) -> Result<()> {
        let collection = self.procs();
        let mut proc = collection.find(pid)?;
        proc.end_time = end_time;
        proc.state = state.into();
        collection.update(&proc)?;
        Ok(())
    }

    pub fn upsert_proc(&self, proc: &Arc<scheduler::Process>) -> Result<()> {
        debug!("upsert process: {}", proc.id());
        let collection = self.procs();
        let data: data::Proc = proc.into_data()?;
        match collection.find(proc.id()) {
            Ok(_) => {
                collection.update(&data)?;
            }
            Err(_) => {
                collection.create(&data)?;
            }
        }

        Ok(())
    }

    fn load_tasks(&self, proc: &Arc<scheduler::Process>, rt: &Arc<Runtime>) -> Result<()> {
        debug!("load_tasks pid={}", proc.id());
        let collection = self.tasks();
        let query = Query::new().filter(Filter::and().expr(Expr::eq("pid", proc.id())));
        let tasks = collection.query(&query)?;
        let tree = &proc.tree();

        // phase 1: load tasks and register dynamic nodes into the tree map,
        // so node links (parent/prev/next) can be resolved afterwards
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

            // resume data
            if !t.data.is_empty() {
                let data = serde_json::from_str(&t.data)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_data(&data);
            }

            // resume task sealed data
            if !t.sealed.is_empty() {
                let data = serde_json::from_str(&t.sealed)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_sealed_data(&data);
            }

            if let Some(err) = t.err {
                let err: Error =
                    serde_json::from_str(&err).map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_err(&err)
            }
            proc.push_task(Arc::new(task))?;
        }

        // phase 2: rebuild the node graph (parent/prev/next) of dynamic nodes
        for (node, data) in dyn_nodes.iter() {
            node.restore_links(data, tree);
        }

        Ok(())
    }
}
