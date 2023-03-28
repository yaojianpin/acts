use crate::{sch::Scheduler, store::Store, ActResult, Message, ModelInfo, ProcInfo, TaskInfo};
use std::sync::Arc;

#[derive(Clone)]
pub struct Manager {
    scher: Arc<Scheduler>,
    store: Arc<Store>,
}

impl Manager {
    pub(crate) fn new(sch: &Arc<Scheduler>, store: &Arc<Store>) -> Self {
        Self {
            scher: sch.clone(),
            store: store.clone(),
        }
    }

    pub fn models(&self, limit: usize) -> ActResult<Vec<ModelInfo>> {
        match self.store.models(limit) {
            Ok(models) => {
                let mut ret = Vec::new();
                for m in models {
                    ret.push(m.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    pub fn model(&self, id: &str) -> ActResult<ModelInfo> {
        match self.store.model(id) {
            Ok(m) => Ok(m.into()),
            Err(err) => Err(err),
        }
    }

    pub fn remove(&self, model_id: &str) -> ActResult<bool> {
        self.store.remove_model(model_id)
    }

    pub fn procs(&self, cap: usize) -> ActResult<Vec<ProcInfo>> {
        match self.store.procs(cap) {
            Ok(ref procs) => {
                let mut ret = Vec::new();
                for t in procs {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    pub fn proc(&self, pid: &str) -> ActResult<ProcInfo> {
        match self.store.proc(pid) {
            Ok(ref proc) => Ok(proc.into()),
            Err(err) => Err(err),
        }
    }

    pub fn tasks(&self, pid: &str) -> ActResult<Vec<TaskInfo>> {
        match self.store.tasks(pid) {
            Ok(tasks) => {
                let mut ret = Vec::new();
                for t in tasks {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    pub fn task(&self, pid: &str, tid: &str) -> ActResult<TaskInfo> {
        match self.store.task(pid, tid) {
            Ok(t) => Ok(t.into()),
            Err(err) => Err(err),
        }
    }

    pub fn messages(&self, pid: &str) -> ActResult<Vec<Message>> {
        match self.store.messages(pid) {
            Ok(tasks) => {
                let mut ret = Vec::new();
                for t in tasks {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    pub fn close(&self, pid: &str) -> ActResult<bool> {
        self.scher.cache().remove(pid)
    }
}
