use crate::Workflow;
use crate::export::RunHandle;
use crate::utils::consts;
use crate::{
    ActError, ModelInfo, Principal, ProcInfo, ProcessGraph, Result, TaskInfo, Vars, query::Query,
    scheduler::Runtime, store::PageData,
};
use std::sync::Arc;
use tracing::instrument;

/// `proc:ls` — list process rows.
pub(crate) const LS: &str = "proc:ls";
/// `proc:get` — read one process row and its in-memory tasks.
pub(crate) const GET: &str = "proc:get";
/// `proc:graph` — read the run's declared model graph (executed or not).
pub(crate) const GRAPH: &str = "proc:graph";
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

    /// Start a deployed model and return its durable handle.
    ///
    /// The run is registered in the aggregate before this returns, so
    /// [`RunHandle::wait`](crate::RunHandle::wait) observes every terminal
    /// state — including one reached before a channel handler subscribed.
    #[instrument(skip(self, options), fields(mid = %mid))]
    pub async fn start(&self, mid: &str, options: Vars) -> Result<RunHandle> {
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
    ) -> Result<RunHandle> {
        // The authority travels inside the start options (set here, never
        // accepted from them) and is popped by `Runtime::start` so it cannot
        // leak into the workflow's user vars.
        options.set(consts::PROC_OWNER, owner);
        let model: ModelInfo = self.runtime.store().models().find(mid).await?.into();
        let workflow = model.workflow()?;
        // The model's resource must be within the owner's grants — the same
        // check a deploy runs, carried by the sealed policy so a subflow
        // cannot widen what its parent may run.
        if !owner.allows_rn(&workflow.rn) {
            return Err(crate::ActError::Denied(format!(
                "resource '{}' of model '{mid}' is not allowed",
                workflow.rn
            )));
        }
        let pid = self.runtime.start(&workflow, options).await?;
        Ok(RunHandle::new(&self.runtime, pid))
    }

    /// Start a model given inline and return its durable handle (see
    /// [`Self::start`]).
    #[instrument(skip(self, model, options), fields(fmt = %fmt))]
    pub async fn start_from_model(
        &self,
        model: &str,
        fmt: &str,
        mut options: Vars,
    ) -> Result<RunHandle> {
        self.principal.check(START_FROM_MODEL)?;
        options.set(consts::PROC_OWNER, self.principal.scope_policy());
        let workflow = match fmt {
            "yaml" | "yml" => Workflow::from_yml(model),
            "json" => Workflow::from_json(model),
            _ => Err(ActError::Model(format!(
                "'{fmt}' is invalid, it must be one of 'yaml' and 'json'"
            ))),
        }?;
        self.principal.check_rn(&workflow.rn)?;
        let pid = self.runtime.start(&workflow, options).await?;
        Ok(RunHandle::new(&self.runtime, pid))
    }

    /// A durable handle to an existing run, by pid — the restart path.
    ///
    /// A handle is only the identity: nothing is loaded here, and a handle
    /// for a pid this runtime cannot see fails on [`RunHandle::wait`] rather
    /// than at construction. Checked as [`GET`], since waiting reads the same
    /// run facts `proc:get` exposes.
    #[instrument(skip(self))]
    pub fn handle(&self, pid: &str) -> Result<RunHandle> {
        self.principal.check(GET)?;
        Ok(RunHandle::new(&self.runtime, pid.to_string()))
    }

    #[instrument(skip(self, q))]
    pub async fn list(&self, q: &Query) -> Result<PageData<ProcInfo>> {
        self.principal.check(LS)?;
        super::list_infos(self.runtime.store().procs(), q).await
    }

    #[instrument(skip(self))]
    pub async fn get(&self, pid: &str) -> Result<ProcInfo> {
        self.principal.check(GET)?;
        // The row query answers with the stored task rows; a missing proc row
        // is the same store error it has always been.
        let proc = self.runtime.store().procs().find(pid).await?;
        let mut info: ProcInfo = (&proc).into();

        if let Some(read) = self.runtime.read_proc(pid).await? {
            let tasks: Vec<TaskInfo> = read
                .tasks()
                .iter()
                .map(|task| {
                    let mut info = TaskInfo::from(task.row());
                    // The scope vars live in the paired vars row; the
                    // lifecycle row alone leaves this blank.
                    info.data = task.data().to_string();
                    info
                })
                .collect();
            info.tasks = tasks;
        }

        Ok(info)
    }

    /// The run's declared model graph (`proc:graph`): every node the run's
    /// durable (materialized) model holds, executed or not, in declaration
    /// order. The node ids are the ones the run's messages carry as `nid`, so
    /// a client renders the full flow (the nodes no execution reached too)
    /// and folds the message stream onto it; the import's virtual container
    /// for a container package's inline acts is kind `block` — the
    /// declaration declares no such node — and the tree display folds it into
    /// its parent step.
    #[instrument(skip(self))]
    pub async fn graph(&self, pid: &str) -> Result<ProcessGraph> {
        self.principal.check(GRAPH)?;
        self.runtime.graph(pid).await
    }
}
