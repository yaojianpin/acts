use crate::{
    ActError, Error, Message, Result, Workflow,
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

    /// Record a durable outbox entry: the task's `next` propagation is enqueued
    /// but not yet run. Deduplicated per `(pid, tid)` — at most one in-flight
    /// record per task, matching the previous `Sign::NEXT_PENDING` semantics.
    /// Called synchronously *before* the in-memory queue dispatch, after the
    /// task state was flushed, so a `Pending` record always has a durable task
    /// behind it.
    pub fn enqueue_next_op(&self, pid: &str, tid: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string()))
                .expr(Expr::eq("status", data::OpStatus::Pending.as_ref())),
        );
        if !collection.query(&q)?.rows.is_empty() {
            return Ok(());
        }

        let now = utils::time::time_millis();
        let op = data::Op {
            id: utils::longid(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            r#type: data::OpType::Next.as_ref().to_string(),
            status: data::OpStatus::Pending.as_ref().to_string(),
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

    /// Close the in-flight outbox records of a task (`Pending` → `Done`).
    /// Must only be called after the operation's effects — including the
    /// `NEXT_COMPLETE` marker — were durably persisted.
    pub fn complete_next_ops(&self, pid: &str, tid: &str) -> Result<()> {
        let collection = self.ops();
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string()))
                .expr(Expr::eq("status", data::OpStatus::Pending.as_ref())),
        );
        for mut op in collection.query(&q)?.rows {
            op.status = data::OpStatus::Done.as_ref().to_string();
            op.update_time = utils::time::time_millis();
            collection.update(&op)?;
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

    pub fn set_message(&self, id: &str, status: MessageStatus) -> Result<()> {
        if let Ok(mut message) = self.messages().find(id) {
            message.status = status;
            message.update_time = utils::time::time_millis();

            self.messages().update(&message)?;
        }

        // it's ok there is no message
        Ok(())
    }

    pub fn set_message_with(&self, pid: &str, tid: &str, status: MessageStatus) -> Result<bool> {
        debug!("set_message_with pid={pid} tid={tid} status={status:?}");
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", tid.to_string())),
        );
        let collection = self.messages();
        if let Ok(messages) = collection.query(&q) {
            for m in messages.rows.iter() {
                let mut m = m.clone();
                m.status = status;
                m.update_time = utils::time::time_millis();
                collection.update(&m)?;
            }
        }

        // it's ok there is no message
        // the message does exist or not depends on the emitter
        // it is allowed the client creates emitter without emit_id
        Ok(true)
    }

    pub fn with_no_response_messages<F: Fn(&Message)>(
        &self,
        timeout_millis: i64,
        max_message_retry_times: i32,
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
        let collection = self.messages();
        if let Ok(messages) = collection.query(&q) {
            for m in messages.rows.iter() {
                let mut message = m.clone();
                message.update_time = utils::time::time_millis();
                if message.retry_times < max_message_retry_times {
                    message.retry_times += 1;
                    let _ = collection.update(&message);
                    f(&message.into());
                } else {
                    // mark the message as error
                    // the error messages will re-send by manual through the manager command
                    message.status = MessageStatus::Error;
                    let _ = collection.update(&message);
                }
            }
        }
        Ok(())
    }

    pub fn resend_error_messages(&self) -> Result<()> {
        let collection = self.messages();
        let q = Query::new().filter(Filter::and().expr(Expr::eq("status", MessageStatus::Error)));
        if let Ok(messages) = collection.query(&q) {
            for m in messages.rows.iter() {
                let mut message = m.clone();
                message.status = MessageStatus::Created;
                message.retry_times = 0;
                message.update_time = utils::time::time_millis();
                collection.update(&message)?;
            }
        }

        Ok(())
    }

    pub fn clear_error_messages(&self, pid: Option<String>) -> Result<()> {
        let collection = self.messages();
        let mut cond = Filter::and().expr(Expr::eq("status", MessageStatus::Error));
        if let Some(pid) = &pid {
            cond = cond.expr(Expr::eq("pid", pid));
        }

        let q = Query::new().filter(cond);
        if let Ok(messages) = collection.query(&q) {
            for m in messages.rows.iter() {
                collection.delete(&m.id)?;
            }
        }

        Ok(())
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
            proc.push_task(Arc::new(task));
        }

        // phase 2: rebuild the node graph (parent/prev/next) of dynamic nodes
        for (node, data) in dyn_nodes.iter() {
            node.restore_links(data, tree);
        }

        Ok(())
    }
}
