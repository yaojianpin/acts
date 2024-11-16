use crate::{
    data::Package,
    sch::Runtime,
    store::{Cond, Expr, StoreAdapter},
    utils::Id,
    MessageInfo, ModelInfo, PackageInfo, ProcInfo, Query, Result, TaskInfo, Workflow,
};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct Manager {
    runtime: Arc<Runtime>,
}

impl Manager {
    pub(crate) fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self))]
    pub fn publish(&self, pack: &Package) -> Result<bool> {
        let ret = self.runtime.cache().store().publish(pack)?;
        Ok(ret)
    }

    pub fn resend_error_messages(&self) -> Result<()> {
        self.runtime.cache().store().resend_error_messages()?;
        Ok(())
    }

    pub fn clear_error_messages(&self) -> Result<()> {
        self.runtime.cache().store().clear_error_messages()?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub fn packages(&self, limit: usize) -> Result<Vec<PackageInfo>> {
        let query = Query::new().set_limit(limit);
        match self.runtime.cache().store().packages().query(&query) {
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
    pub fn package(&self, id: &str) -> Result<PackageInfo> {
        let package = &self.runtime.cache().store().packages().find(id)?;
        Ok(package.into())
    }

    #[instrument(skip(self))]
    pub fn deploy(&self, model: &Workflow) -> Result<bool> {
        model.valid()?;
        let ret = self.runtime.cache().store().deploy(model)?;
        Ok(ret)
    }

    #[instrument(skip(self))]
    pub fn models(&self, limit: usize) -> Result<Vec<ModelInfo>> {
        let query = Query::new().set_limit(limit);
        match self.runtime.cache().store().models().query(&query) {
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
        match self.runtime.cache().store().models().find(id) {
            Ok(m) => {
                let mut model: ModelInfo = m.into();
                if fmt == "tree" {
                    let workflow = Workflow::from_yml(&model.data)?;
                    model.data = workflow.tree_output();
                }
                Ok(model)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn rm_model(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().models().delete(id)
    }

    #[instrument(skip(self))]
    pub fn rm_package(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().packages().delete(id)
    }

    #[instrument(skip(self))]
    pub fn rm_message(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().messages().delete(id)
    }

    #[instrument(skip(self))]
    pub fn procs(&self, cap: usize) -> Result<Vec<ProcInfo>> {
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
    pub fn proc(&self, pid: &str) -> Result<ProcInfo> {
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

    #[instrument(skip(self))]
    pub fn tasks(&self, pid: &str, count: usize) -> Result<Vec<TaskInfo>> {
        let query = Query::new()
            .push(Cond::and().push(Expr::eq("pid", pid.to_string())))
            .set_limit(10000);
        match self.runtime.cache().store().tasks().query(&query) {
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
                .push(Expr::eq("pid", pid.to_string()))
                .push(Expr::eq("kind", "act")),
        );
        match self.runtime.cache().store().tasks().query(&query) {
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
    pub fn messages(self: &Arc<Self>, pid: &str, count: usize) -> Result<Vec<MessageInfo>> {
        let query = Query::new()
            .push(Cond::and().push(Expr::eq("pid", pid.to_string())))
            .set_limit(10000);
        match self.runtime.cache().store().messages().query(&query) {
            Ok(mut messages) => {
                let mut ret = Vec::new();
                messages.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                for t in messages.iter().take(count) {
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
        match self.runtime.cache().store().tasks().find(&id.id()) {
            Ok(t) => Ok(t.into()),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn message(self: &Arc<Self>, id: &str) -> Result<MessageInfo> {
        let message = &self.runtime.cache().store().messages().find(id)?;
        Ok(message.into())
    }
}
