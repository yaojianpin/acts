use crate::Workflow;
use crate::scheduler::Process;
use crate::{
    ActError, ModelInfo, ProcInfo, Result, TaskInfo, Vars, query::Query, scheduler::Runtime,
    store::PageData,
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

    pub fn start(&self, mid: &str, options: Vars) -> Result<String> {
        let model: ModelInfo = self.runtime.cache().store().models().find(mid)?.into();
        let workflow = model.workflow()?;
        let proc = self.runtime.start(&workflow, options)?;
        Ok(proc.id().to_string())
    }

    pub fn start_from_model(&self, model: &str, fmt: &str, options: Vars) -> Result<String> {
        let workflow = match fmt {
            "yaml" | "yml" => Workflow::from_yml(model),
            "json" => Workflow::from_json(model),
            _ => Err(ActError::Model(format!(
                "'{fmt}' is invalid, it must be one of 'yaml' and 'json'"
            ))),
        }?;
        let proc = self.runtime.start(&workflow, options)?;
        Ok(proc.id().to_string())
    }

    #[instrument(skip(self))]
    pub fn list(&self, q: &Query) -> Result<PageData<ProcInfo>> {
        match self.runtime.cache().store().procs().query(q) {
            Ok(procs) => Ok(PageData {
                count: procs.count,
                page_size: procs.page_size,
                page_count: procs.page_count,
                page_num: procs.page_num,
                rows: procs.rows.iter().map(ProcInfo::from).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, pid: &str) -> Result<ProcInfo> {
        match self.runtime.cache().store().procs().find(pid) {
            Ok(ref proc) => {
                let mut info: ProcInfo = proc.into();

                if let Some(proc) = self.runtime.proc(pid)? {
                    let mut tasks: Vec<TaskInfo> =
                        proc.tasks().iter().map(TaskInfo::from).collect();

                    tasks.sort_by_key(|a| a.timestamp);
                    info.tasks = tasks;
                }

                Ok(info)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get_process(&self, pid: &str) -> Result<Option<Arc<Process>>> {
        self.runtime.proc(pid)
    }
}
