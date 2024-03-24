use tracing::instrument;

use crate::{
    data::Package,
    sch::Scheduler,
    store::{Cond, Expr, StoreAdapter},
    utils::Id,
    ActionResult, ModelInfo, PackageInfo, ProcInfo, Query, Result, TaskInfo, Workflow,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Manager {
    scher: Arc<Scheduler>,
}

impl Manager {
    pub(crate) fn new(sch: &Arc<Scheduler>) -> Self {
        Self { scher: sch.clone() }
    }

    #[instrument(skip(self))]
    pub fn publish(&self, pack: &Package) -> Result<ActionResult> {
        let mut state = ActionResult::begin();
        self.scher.cache().store().publish(pack)?;
        state.end();

        Ok(state)
    }

    #[instrument(skip(self))]
    pub fn packages(&self, limit: usize) -> Result<Vec<PackageInfo>> {
        let query = Query::new().set_limit(limit);
        match self.scher.cache().store().packages().query(&query) {
            Ok(mut packages) => {
                packages.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                let mut ret = Vec::new();
                for t in &packages {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn deploy(&self, model: &Workflow) -> Result<ActionResult> {
        let mut state = ActionResult::begin();
        model.valid()?;
        self.scher.cache().store().deploy(model)?;
        state.end();

        Ok(state)
    }

    #[instrument(skip(self))]
    pub fn models(&self, limit: usize) -> Result<Vec<ModelInfo>> {
        let query = Query::new().set_limit(limit);
        match self.scher.cache().store().models().query(&query) {
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

    #[instrument(skip(self))]
    pub fn model(&self, id: &str, fmt: &str) -> Result<ModelInfo> {
        match self.scher.cache().store().models().find(id) {
            Ok(m) => {
                let mut model: ModelInfo = m.into();
                if fmt == "tree" {
                    let workflow = Workflow::from_yml(&model.model)?;
                    model.model = workflow.tree_output();
                }
                Ok(model)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn remove(&self, model_id: &str) -> Result<bool> {
        self.scher.cache().store().models().delete(model_id)
    }

    #[instrument(skip(self))]
    pub fn procs(&self, cap: usize) -> Result<Vec<ProcInfo>> {
        let query = Query::new().set_limit(cap);
        match self.scher.cache().store().procs().query(&query) {
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
    pub fn proc(&self, pid: &str, fmt: &str) -> Result<ProcInfo> {
        match self.scher.cache().store().procs().find(pid) {
            Ok(ref proc) => {
                let mut info: ProcInfo = proc.into();

                if let Some(proc) = self.scher.proc(pid) {
                    if fmt == "tree" {
                        info.tasks = proc.tree_output();
                    } else if fmt == "json" {
                        let mut tasks: Vec<TaskInfo> = Vec::new();
                        for task in proc.tasks().iter() {
                            tasks.push(task.into());
                        }

                        tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                        info.tasks = serde_json::to_string_pretty(&tasks)
                            .unwrap_or_else(|err| err.to_string());
                    }
                }

                Ok(info)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn tasks(&self, pid: &str, count: usize) -> Result<Vec<TaskInfo>> {
        let query = Query::new()
            .push(Cond::and().push(Expr::eq("proc_id", pid)))
            .set_limit(10000);
        match self.scher.cache().store().tasks().query(&query) {
            Ok(mut tasks) => {
                let mut ret = Vec::new();
                tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                for t in tasks.into_iter().take(count) {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn acts(&self, pid: &str) -> Result<Vec<TaskInfo>> {
        let query = Query::new().push(
            Cond::and()
                .push(Expr::eq("proc_id", pid))
                .push(Expr::eq("kind", "act".into())),
        );
        match self.scher.cache().store().tasks().query(&query) {
            Ok(mut tasks) => {
                let mut ret = Vec::new();
                tasks.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                for t in tasks {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn task(&self, pid: &str, tid: &str) -> Result<TaskInfo> {
        let id = Id::new(pid, tid);
        match self.scher.cache().store().tasks().find(&id.id()) {
            Ok(t) => Ok(t.into()),
            Err(err) => Err(err),
        }
    }
}
