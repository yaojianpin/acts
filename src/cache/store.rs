use crate::{
    data::{self, MessageStatus},
    sch::{self, Node, Runtime, StatementBatch, TaskLifeCycle, TaskState},
    store::{Cond, Expr, Query, Store},
    utils::{self, Id},
    ActError, Error, Message, Result, StoreAdapter, Workflow,
};
use std::{collections::HashMap, sync::Arc};
use tracing::debug;

impl Store {
    pub fn load(&self, cap: usize, rt: &Arc<Runtime>) -> Result<Vec<Arc<sch::Proc>>> {
        debug!("load cap={}", cap);
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new()
                .push(
                    Cond::or()
                        .push(Expr::eq("state", TaskState::None.to_string()))
                        .push(Expr::eq("state", TaskState::Ready.to_string()))
                        .push(Expr::eq("state", TaskState::Running.to_string()))
                        .push(Expr::eq("state", TaskState::Pending.to_string())),
                )
                .set_limit(cap);
            let procs = self.procs().query(&query)?;
            for p in procs {
                let model = Workflow::from_json(&p.model)?;
                let env_local: serde_json::Value = serde_json::from_str(&p.env_local)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                let state = p.state.clone();
                let proc = sch::Proc::new_with_timestamp(&p.id, p.timestamp, rt);

                proc.load(&model)?;
                proc.set_pure_state(state.into());
                proc.set_start_time(p.start_time);
                proc.set_end_time(p.end_time);
                proc.set_env_local(&env_local.into());
                if let Some(err) = p.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    proc.set_pure_err(&err)
                }

                self.load_tasks(&proc, rt)?;
                ret.push(proc);
            }
        }

        Ok(ret)
    }

    pub fn load_proc(&self, pid: &str, rt: &Arc<Runtime>) -> Result<Option<Arc<sch::Proc>>> {
        debug!("load proc pid={}", pid);
        match self.procs().find(pid) {
            Ok(p) => {
                // println!("proc model={}", p.model);
                let model = Workflow::from_json(&p.model)?;
                let proc = sch::Proc::new(pid, rt);
                let env_local: serde_json::Value = serde_json::from_str(&p.env_local)
                    .map_err(|err| ActError::Store(err.to_string()))?;

                proc.load(&model)?;
                proc.set_pure_state(p.state.into());
                proc.set_start_time(p.start_time);
                proc.set_env_local(&env_local.into());
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
        let q = Query::new().push(Cond::and().push(Expr::eq("pid", pid.to_string())));
        let tasks = self.tasks().query(&q)?;
        for task in tasks {
            self.tasks().delete(&task.id)?;
        }
        self.procs().delete(pid)?;
        Ok(true)
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
        let q = Query::new().push(
            Cond::and()
                .push(Expr::eq("pid", pid.to_string()))
                .push(Expr::eq("tid", tid.to_string())),
        );
        if let Ok(messages) = self.messages().query(&q) {
            for m in messages.iter() {
                let mut m = m.clone();
                m.status = status;
                m.update_time = utils::time::time_millis();
                self.messages().update(&m)?;
            }
        }

        // it's ok there is no message
        // the message does exist or not depends on the emitter
        // it is allowed the client creates emitter without emit_id
        Ok(true)
    }

    pub fn with_no_response_messages<F: Fn(&Message)>(
        &self,
        timeout_millis: u64,
        max_message_retry_times: i32,
        f: F,
    ) {
        let q = Query::new().set_limit(300).push(
            Cond::and()
                .push(Expr::eq("status", MessageStatus::Created))
                .push(Expr::lt(
                    "update_time",
                    utils::time::time_millis() as u64 - timeout_millis,
                )),
        );
        if let Ok(messages) = self.messages().query(&q) {
            for m in messages.iter() {
                let mut message = m.clone();
                message.update_time = utils::time::time_millis();
                if message.retry_times < max_message_retry_times {
                    message.retry_times += 1;
                    let _ = self.messages().update(&message);
                    f(&message.into());
                } else {
                    // mark the message as error
                    // the error messages will re-send by manual through the manager command
                    message.status = MessageStatus::Error;
                    let _ = self.messages().update(&message);
                }
            }
        }
    }

    pub fn resend_error_messages(&self) -> Result<()> {
        let q = Query::new().push(Cond::and().push(Expr::eq("status", MessageStatus::Error)));
        if let Ok(messages) = self.messages().query(&q) {
            for m in messages.iter() {
                let mut message = m.clone();
                message.status = MessageStatus::Created;
                message.retry_times = 0;
                message.update_time = utils::time::time_millis();
                self.messages().update(&message)?;
            }
        }

        Ok(())
    }

    pub fn clear_error_messages(&self) -> Result<()> {
        let q = Query::new().push(Cond::and().push(Expr::eq("status", MessageStatus::Error)));
        if let Ok(messages) = self.messages().query(&q) {
            for m in messages.iter() {
                self.messages().delete(&m.id)?;
            }
        }

        Ok(())
    }

    pub fn upsert_task(&self, task: &Arc<sch::Task>) -> Result<()> {
        debug!("upsert_task: {task:?}");
        let data: data::Task = task.into_data()?;
        let id = Id::new(&task.pid, &task.id);
        match self.tasks().find(&id.id()) {
            Ok(_) => {
                self.tasks().update(&data)?;
            }
            Err(_) => {
                self.tasks().create(&data)?;
            }
        }

        Ok(())
    }

    pub fn upsert_proc(&self, proc: &Arc<sch::Proc>) -> Result<()> {
        debug!("upsert proc: {}", proc.id());
        let data: data::Proc = proc.into_data()?;
        match self.procs().find(proc.id()) {
            Ok(_) => {
                self.procs().update(&data)?;
            }
            Err(_) => {
                self.procs().create(&data)?;
            }
        }

        Ok(())
    }

    fn load_tasks(&self, proc: &Arc<sch::Proc>, rt: &Arc<Runtime>) -> Result<()> {
        debug!("load_tasks pid={}", proc.id());
        let tree = &proc.tree();
        let query = Query::new().push(Cond::and().push(Expr::eq("pid", proc.id())));
        let tasks = self.tasks().query(&query)?;
        for t in tasks {
            let state: TaskState = t.state.into();
            let node = Node::from_str(&t.node_data, tree);
            let mut task = sch::Task::new(proc, &t.tid, node, rt);
            task.set_pure_state(state.clone());
            task.set_start_time(t.start_time);
            task.set_end_time(t.end_time);
            task.timestamp = t.timestamp;
            task.set_prev(t.prev);

            let data =
                serde_json::from_str(&t.data).map_err(|err| ActError::Store(err.to_string()))?;
            task.set_data(&data);

            let hooks: HashMap<TaskLifeCycle, Vec<StatementBatch>> =
                serde_json::from_str(&t.hooks).map_err(|err| ActError::Store(err.to_string()))?;

            task.set_hooks(&hooks);
            if let Some(err) = t.err {
                let err: Error =
                    serde_json::from_str(&err).map_err(|err| ActError::Store(err.to_string()))?;
                task.set_pure_err(&err)
            }
            // cache.push(proc)
            // cache.push_task_pri(&Arc::new(task), false)?;
            proc.push_task(Arc::new(task));
        }

        Ok(())
    }
}
