use super::ExecutorQuery;
use crate::{
    scheduler::Runtime,
    store::{PageData, StoreAdapter},
    utils::consts,
    ModelInfo, ProcInfo, Result, TaskInfo, Vars,
};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct ProcessExecutor {
    runtime: Arc<Runtime>,
}

impl ProcessExecutor {
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
    pub fn list(&self, q: &ExecutorQuery) -> Result<PageData<ProcInfo>> {
        let query = q.into_query();
        match self.runtime.cache().store().procs().query(&query) {
            Ok(procs) => Ok(PageData {
                count: procs.count,
                page_size: procs.page_size,
                page_count: procs.page_count,
                page_num: procs.page_num,
                rows: procs.rows.iter().map(|m| m.into()).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, pid: &str) -> Result<ProcInfo> {
        match self.runtime.cache().store().procs().find(pid) {
            Ok(ref proc) => {
                let mut info: ProcInfo = proc.into();

                if let Some(proc) = self.runtime.cache().proc(pid, &self.runtime) {
                    let mut tasks: Vec<TaskInfo> = proc.tasks().iter().map(TaskInfo::from).collect();

                    tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                    info.tasks = tasks;
                }

                Ok(info)
            }
            Err(err) => Err(err),
        }
    }
}
