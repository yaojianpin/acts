use crate::{
    sch::{self, NodeKind, Scheduler, TaskState},
    store::{Query, Store},
    utils, ActResult, StoreAdapter, Workflow,
};
use std::sync::Arc;

impl Store {
    pub fn load(&self, cap: usize) -> Vec<Arc<sch::Proc>> {
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new()
                .push("state", &TaskState::Running.to_string())
                .set_limit(cap);
            let procs = self.procs().query(&query).expect("get store procs");
            for p in procs {
                let workflow = Workflow::from_str(&p.model).unwrap();
                let vars = &utils::vars::from_string(&p.vars);
                let state = p.state.clone();
                let proc = Arc::new(sch::Proc::new_raw(&workflow, &p.pid, &state.into()));
                proc.append_vars(vars);

                self.load_tasks(&proc);
                self.load_acts(&proc);

                ret.push(proc);
            }
        }

        ret
    }

    pub fn load_proc(&self, pid: &str, scher: &Scheduler) -> Option<Arc<sch::Proc>> {
        match self.procs().find(pid) {
            Ok(p) => {
                let workflow = Workflow::from_str(&p.model).unwrap();
                let mut proc = Arc::new(scher.create_raw_proc(pid, &workflow));
                proc.set_state(p.state.into());
                self.load_tasks(&mut proc);
                self.load_acts(&mut proc);
                return Some(proc);
            }
            Err(_) => None,
        }
    }

    pub fn remove_proc(&self, pid: &str) -> ActResult<bool> {
        self.procs().delete(pid)?;

        let q = Query::new().push("pid", pid);
        let tasks = self.tasks().query(&q)?;
        for task in tasks {
            self.tasks().delete(&task.id)?;
        }

        let acts = self.acts().query(&q)?;
        for act in acts {
            self.acts().delete(&act.id)?;
        }
        Ok(true)
    }
    fn load_tasks(&self, proc: &Arc<sch::Proc>) {
        let query = Query::new().push("pid", &proc.pid());
        let tasks = self.tasks().query(&query).expect("get proc tasks");
        for t in tasks {
            let kind: NodeKind = t.kind.into();
            let state: TaskState = t.state.into();
            if kind == NodeKind::Workflow {
                proc.set_state(state.clone());
            }

            if let Some(node) = proc.node(&t.nid) {
                let task = sch::Task::new(&proc, &t.tid, node);
                task.set_pure_state(state.clone());
                task.set_start_time(t.start_time);
                task.set_end_time(t.end_time);
                proc.push_task(Arc::new(task));
            }
        }
    }

    fn load_acts(&self, proc: &Arc<sch::Proc>) {
        let query = Query::new().push("pid", &proc.pid());
        let acts = self.acts().query(&query).expect("get proc acts");
        for store_act in acts {
            if let Some(task) = proc.task(&store_act.tid) {
                let vars = utils::vars::from_string(&store_act.vars);
                let kind = store_act.kind.as_str().into();
                let state: TaskState = store_act.state.into();

                let act = sch::Act::new_with_id(&task, kind, &store_act.id, &vars);
                act.set_pure_state(state);
                act.set_start_time(store_act.start_time);
                act.set_end_time(store_act.start_time);
                act.set_active(store_act.active);

                proc.push_act(&act);
            }
        }
    }
}
