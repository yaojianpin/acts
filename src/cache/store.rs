use tracing::debug;

use crate::{
    event::ActionState,
    sch::{self, TaskState},
    store::{Cond, Expr, Query, Store},
    utils, ActError, Result, StoreAdapter, Workflow,
};
use std::sync::Arc;

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
                let vars = &utils::vars::from_string(&p.vars);
                let state = p.state.clone();
                let mut proc = sch::Proc::new(&p.id);
                proc.load(&model)?;
                proc.set_pure_state(state.into());
                proc.set_start_time(p.start_time);
                proc.set_end_time(p.end_time);
                proc.set_timestamp(p.timestamp);
                proc.append_vars(vars);

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
                proc.load(&model)?;
                proc.set_state(p.state.into());
                self.load_tasks(&mut proc)?;

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
            let action_state: ActionState = t.action_state.into();
            if let Some(node) = proc.node(&t.node_id) {
                let mut task = sch::Task::new(&proc, &t.task_id, node);
                task.set_pure_state(state.clone());
                task.set_pure_action_state(action_state);
                task.set_start_time(t.start_time);
                task.set_end_time(t.end_time);
                task.timestamp = t.timestamp;
                task.set_prev(t.prev);

                let vars = serde_json::from_str(&t.vars)
                    .map_err(|err| ActError::Store(err.to_string()))?;
                task.room().append(&vars);
                proc.push_task(Arc::new(task));
            }
        }

        Ok(())
    }
}
