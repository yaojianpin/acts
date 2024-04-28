use crate::{
    sch::{self, StatementBatch, TaskLifeCycle, TaskState},
    store::{Cond, Expr, Query, Store},
    ActError, Error, Result, StoreAdapter, Workflow,
};
use std::{collections::HashMap, sync::Arc};
use tracing::debug;

impl Store {
    pub fn load(&self, cap: usize) -> Result<Vec<Arc<sch::Proc>>> {
        debug!("load cap={}", cap);
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new()
                .push(
                    Cond::or()
                        .push(Expr::eq("state", &TaskState::None.to_string()))
                        .push(Expr::eq("state", &TaskState::Running.to_string()))
                        .push(Expr::eq("state", &TaskState::Pending.to_string())),
                )
                .set_limit(cap);
            let procs = self.procs().query(&query)?;
            for p in procs {
                let model = Workflow::from_json(&p.model)?;
                let env_local: serde_json::Value = serde_json::from_str(&p.env_local)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                let state = p.state.clone();
                let mut proc = sch::Proc::new(&p.id);
                proc.load(&model)?;
                proc.set_pure_state(state.into());
                proc.set_start_time(p.start_time);
                proc.set_end_time(p.end_time);
                proc.set_timestamp(p.timestamp);
                proc.set_env_local(&env_local.into());
                if let Some(err) = p.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    proc.set_pure_err(&err)
                }

                let proc = Arc::new(proc);
                self.load_tasks(&proc)?;
                ret.push(proc);
            }
        }

        Ok(ret)
    }

    pub fn load_proc(&self, pid: &str) -> Result<Option<Arc<sch::Proc>>> {
        debug!("load_proc proc={}", pid);
        match self.procs().find(pid) {
            Ok(p) => {
                let model = Workflow::from_json(&p.model)?;
                let mut proc = Arc::new(sch::Proc::new(pid));
                let env_local: serde_json::Value = serde_json::from_str(&p.env_local)
                    .map_err(|err| ActError::Store(err.to_string()))?;

                proc.load(&model)?;
                proc.set_state(p.state.into());
                proc.set_root_tid(&p.root_tid);
                proc.set_env_local(&env_local.into());
                self.load_tasks(&mut proc)?;
                if let Some(err) = p.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    proc.set_pure_err(&err)
                }
                return Ok(Some(proc));
            }
            Err(_) => Ok(None),
        }
    }

    pub fn remove_proc(&self, pid: &str) -> Result<bool> {
        debug!("remove_proc pid={}", pid);
        self.procs().delete(pid)?;
        let q = Query::new().push(Cond::and().push(Expr::eq("proc_id", pid)));
        let tasks = self.tasks().query(&q)?;
        for task in tasks {
            self.tasks().delete(&task.id)?;
        }

        Ok(true)
    }
    fn load_tasks(&self, proc: &Arc<sch::Proc>) -> Result<()> {
        debug!("load_tasks pid={}", proc.id());
        let query = Query::new().push(Cond::and().push(Expr::eq("proc_id", &proc.id())));
        let tasks = self.tasks().query(&query)?;
        for t in tasks {
            let state: TaskState = t.state.into();
            if let Some(node) = proc.node(&t.node_id) {
                let mut task = sch::Task::new(&proc, &t.task_id, node);
                task.set_pure_state(state.clone());
                task.set_start_time(t.start_time);
                task.set_end_time(t.end_time);
                task.timestamp = t.timestamp;
                task.set_prev(t.prev);

                let data = serde_json::from_str(&t.data)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.set_data(&data);

                let hooks: HashMap<TaskLifeCycle, Vec<StatementBatch>> =
                    serde_json::from_str(&t.hooks)
                        .map_err(|err| ActError::Store(err.to_string()))?;

                task.set_hooks(&hooks);
                if let Some(err) = t.err {
                    let err: Error = serde_json::from_str(&err)
                        .map_err(|err| ActError::Store(err.to_string()))?;
                    task.set_pure_err(&err)
                }

                proc.push_task(Arc::new(task));
            }
        }

        Ok(())
    }
}
