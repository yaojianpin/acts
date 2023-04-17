use crate::{
    sch::{self, NodeKind, Scheduler, TaskState},
    store::{Query, Store},
    utils, StoreAdapter, Workflow,
};
use std::sync::Arc;

impl Store {
    pub fn load(&self, scher: Arc<Scheduler>, cap: usize) -> Vec<Arc<sch::Proc>> {
        let mut ret = Vec::new();
        if cap > 0 {
            let query = Query::new().set_limit(cap);
            let procs = self.procs().query(&query).expect("get store procs");
            for p in procs {
                let workflow = Workflow::from_str(&p.model).unwrap();
                let vars = &utils::vars::from_string(&p.vars);
                let state = p.state.clone();
                let mut proc =
                    sch::Proc::new_raw(scher.clone(), &workflow, &p.pid, &state.into(), vars);

                self.load_tasks(&mut proc);
                self.load_messages(&mut proc);

                ret.push(Arc::new(proc));
            }
        }

        ret
    }

    pub fn load_proc(&self, pid: &str, scher: &Scheduler) -> Option<Arc<sch::Proc>> {
        match self.procs().find(pid) {
            Ok(p) => {
                let workflow = Workflow::from_str(&p.model).unwrap();
                let mut proc = scher.create_raw_proc(pid, &workflow);
                proc.set_state(&p.state.into());
                self.load_tasks(&mut proc);
                self.load_messages(&mut proc);
                return Some(Arc::new(proc));
            }
            Err(_) => None,
        }
    }

    fn load_tasks(&self, proc: &mut sch::Proc) {
        let query = Query::new().push("pid", &proc.pid());
        let tasks = self.tasks().query(&query).expect("get proc tasks");
        for t in tasks {
            let kind: NodeKind = t.kind.into();
            let state: TaskState = t.state.into();
            if kind == NodeKind::Workflow {
                proc.set_state(&state);
            }

            if let Some(node) = proc.node(&t.nid) {
                let task = sch::Task::new(&proc, &t.tid, node);
                task.set_state(&state);
                task.set_start_time(t.start_time);
                task.set_end_time(t.end_time);
                if !t.uid.is_empty() {
                    task.set_uid(&t.uid);
                }
                proc.push_task(Arc::new(task));
            }
        }
    }

    fn load_messages(&self, proc: &mut sch::Proc) {
        let query = Query::new().push("pid", &proc.pid());
        let messages = self.messages().query(&query).expect("get proc tasks");
        for m in messages {
            let uid = if m.uid.is_empty() { None } else { Some(m.uid) };
            proc.make_message(&m.tid, uid, utils::vars::from_string(&m.vars));
        }
    }
}
