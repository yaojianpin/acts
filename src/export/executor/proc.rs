use crate::{
    sch::Runtime, store::StoreAdapter, utils::consts, ModelInfo, ProcInfo, Query, Result, TaskInfo,
    Vars,
};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct ProcExecutor {
    runtime: Arc<Runtime>,
}

impl ProcExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    pub fn start(&self, mid: &str, options: &Vars) -> Result<String> {
        let model: ModelInfo = self.runtime.cache().store().models().find(mid)?.into();
        let workflow = model.workflow()?;

        let mut vars = options.clone();
        // set the workflow initiator
        if let Some(uid) = options.get_value(consts::FOR_ACT_KEY_UID) {
            vars.insert(consts::INITIATOR.to_string(), uid.clone());
        }
        let proc = self.runtime.start(&workflow, &vars)?;
        Ok(proc.id().to_string())
    }

    #[instrument(skip(self))]
    pub fn list(&self, cap: usize) -> Result<Vec<ProcInfo>> {
        let query = Query::new().set_limit(cap);
        match self.runtime.cache().store().procs().query(&query) {
            Ok(mut procs) => {
                procs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                let mut ret = Vec::new();
                for t in &procs {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, pid: &str) -> Result<ProcInfo> {
        match self.runtime.cache().store().procs().find(pid) {
            Ok(ref proc) => {
                let mut info: ProcInfo = proc.into();

                if let Some(proc) = self.runtime.cache().proc(pid, &self.runtime) {
                    let mut tasks: Vec<TaskInfo> = Vec::new();
                    for task in proc.tasks().iter() {
                        tasks.push(task.into());
                    }

                    tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                    info.tasks = tasks;
                }

                Ok(info)
            }
            Err(err) => Err(err),
        }
    }
}
