use crate::Workflow;
use crate::scheduler::Process;
use crate::utils::consts;
use crate::{
    ActError, ModelInfo, Principal, ProcInfo, Result, TaskInfo, Vars, query::Query, scheduler::Runtime,
    store::PageData,
};
use std::sync::Arc;
use tracing::instrument;

/// `proc:ls` — list process rows.
pub(crate) const LS: &str = "proc:ls";
/// `proc:get` — read one process row and its in-memory tasks.
pub(crate) const GET: &str = "proc:get";
/// `proc:start` — start a deployed model.
pub(crate) const START: &str = "proc:start";
/// `proc:start_from_model` — start a model given inline.
pub(crate) const START_FROM_MODEL: &str = "proc:start_from_model";

#[derive(Debug, Clone)]
pub struct ProcessExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl ProcessExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
        }
    }

    #[instrument(skip(self, options), fields(mid = %mid))]
    pub async fn start(&self, mid: &str, options: Vars) -> Result<String> {
        self.principal.check(START)?;
        // The run inherits the caller's authority — the snapshot scopes it may
        // read and the workdir root its directory is made under — and never a
        // wider one, so two callers of the same model read what they own.
        self.start_as_owner(mid, options, &self.principal.scope_policy())
            .await
    }

    /// Start a run that carries `owner` as its authority, without checking an
    /// action: for the engine's own starts, where there is no caller to check.
    /// Only one such start exists — a subflow, which must inherit the
    /// authority of the run that spawned it (see `package::core::subflow`) —
    /// and inheriting is the point: the child cannot read more than its
    /// parent already could.
    #[instrument(skip(self, options, owner), fields(mid = %mid))]
    pub(crate) async fn start_as_owner(
        &self,
        mid: &str,
        mut options: Vars,
        owner: &crate::ScopePolicy,
    ) -> Result<String> {
        // The authority travels inside the start options (set here, never
        // accepted from them) and is popped by `Runtime::start` so it cannot
        // leak into the workflow's user vars.
        options.set(consts::PROC_OWNER, owner);
        let model: ModelInfo = self
            .runtime
            .cache()
            .store()
            .models()
            .find(mid)
            .await?
            .into();
        let workflow = model.workflow()?;
        let proc = self.runtime.start(&workflow, options).await?;
        Ok(proc.id().to_string())
    }

    #[instrument(skip(self, model, options), fields(fmt = %fmt))]
    pub async fn start_from_model(
        &self,
        model: &str,
        fmt: &str,
        mut options: Vars,
    ) -> Result<String> {
        self.principal.check(START_FROM_MODEL)?;
        options.set(consts::PROC_OWNER, self.principal.scope_policy());
        let workflow = match fmt {
            "yaml" | "yml" => Workflow::from_yml(model),
            "json" => Workflow::from_json(model),
            _ => Err(ActError::Model(format!(
                "'{fmt}' is invalid, it must be one of 'yaml' and 'json'"
            ))),
        }?;
        let proc = self.runtime.start(&workflow, options).await?;
        Ok(proc.id().to_string())
    }

    #[instrument(skip(self, q))]
    pub async fn list(&self, q: &Query) -> Result<PageData<ProcInfo>> {
        self.principal.check(LS)?;
        match self.runtime.cache().store().procs().query(q).await {
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
    pub async fn get(&self, pid: &str) -> Result<ProcInfo> {
        self.principal.check(GET)?;
        let proc = self.runtime.cache().store().procs().find(pid).await?;
        let mut info: ProcInfo = (&proc).into();

        if let Some(proc) = self.runtime.proc(pid).await? {
            let mut tasks: Vec<TaskInfo> = proc.tasks().iter().map(TaskInfo::from).collect();

            tasks.sort_by_key(|a| a.timestamp);
            info.tasks = tasks;
        }

        Ok(info)
    }

    /// The live process object behind a pid, for the embedder that wants to
    /// walk its tree. Checked as [`GET`]: it hands out the same row's data,
    /// only in its resident form.
    #[instrument(skip(self))]
    pub async fn get_process(&self, pid: &str) -> Result<Option<Arc<Process>>> {
        self.principal.check(GET)?;
        self.runtime.proc(pid).await
    }
}
