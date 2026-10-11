//! The effect seam: running a Func package's work inside the host's
//! `ApplyAction` hop.
//!
//! The shadow auto-completes; a runner attached to the host makes the hop
//! execute the step's package for real and fold its outputs into the same
//! atomic commit that completes the execution. The contract is the legacy
//! engine's own: the effect runs before the state commits (at-least-once —
//! a crash between effect and commit re-runs the package on recovery,
//! exactly like the legacy `exec` → persist ordering).
//!
//! The runner drives the package through a *headless* scope: a bare
//! in-memory [`Process`] over the declaring model with one task on the
//! acting node. Packages read the context they always read — the params
//! resolve through the same `fill_params` path — and nothing the headless
//! scope touches is durably written: outputs come back as values and land
//! in the aggregate through the hop's own plan.

use crate::{ActRunAs, Result, Vars, model::Workflow, scheduler::Process, scheduler::Runtime};
use std::sync::Arc;

/// One package effect request: everything the runner needs besides the
/// engine handles it was built with.
pub(crate) struct EffectRequest {
    pub(crate) process_id: String,
    /// The acting node id (the kernel execution's task id).
    pub(crate) task_id: String,
    /// The step's `uses` package id.
    pub(crate) package: String,
    /// The declaring model, for the headless scope's tree. Params resolve on
    /// the headless scope exactly like the legacy run: `task.params` reads
    /// the model node's own declaration through the same fill path.
    pub(crate) model: Workflow,
}

/// Runs one package effect. Attach with
/// [`KernelHost::attach_effect_runner`](super::KernelHost::attach_effect_runner);
/// without one the host auto-completes (the shadow contract).
#[async_trait::async_trait]
pub(crate) trait EffectRunner: Send + Sync {
    /// The package's outputs as scope vars, or `None` when it produced none.
    /// An error fails the hop: the execution is decided `Failed` with the
    /// error variables, and the error walk takes over — the legacy
    /// package-error path.
    async fn run(&self, request: EffectRequest) -> Result<Option<Vars>>;
}

/// Runs real packages through the engine's registry and a headless scope.
pub(crate) struct PackageEffectRunner {
    runtime: Arc<Runtime>,
}

impl PackageEffectRunner {
    pub(crate) fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl EffectRunner for PackageEffectRunner {
    async fn run(&self, request: EffectRequest) -> Result<Option<Vars>> {
        // The same gate the legacy act run applies: only Func packages
        // execute inside the run; Irq waits for the client and Msg only
        // emits.
        let definition = self.runtime.package_definition(&request.package).await?;
        if definition.run_as() != ActRunAs::Func {
            return Ok(None);
        }

        // The headless scope: a bare process over the declaring model with
        // one task on the acting node. Never admitted, never persisted —
        // packages see the context shape they always saw.
        let process = Process::new(&request.process_id, &self.runtime);
        process.load(&request.model)?;
        let node = process
            .tree()
            .node(&request.task_id)
            .ok_or_else(|| {
                crate::ActError::Runtime(format!("effect: model has no node '{}'", request.task_id))
            })?
            .clone();
        let task = process.create_task(&node, None)?;
        let ctx = process.create_context(&task);

        // Params resolve through the same fill path the legacy run uses.
        let params = task.params(&ctx);
        definition.validate(&params)?;

        let package = self
            .runtime
            .package()
            .create(definition.id(), self.runtime.config())?;
        package.execute(&ctx, &params).await
    }
}

/// Turns package outputs into aggregate variable writes (the aggregate's
/// string-valued representation). The caller scopes them to the acting
/// execution.
pub(crate) fn outputs_as_writes(outputs: &Vars) -> Vec<(String, String)> {
    outputs
        .iter()
        .map(|(name, value)| (name.clone(), value.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output projection is the aggregate's variable representation:
    /// JSON-encoded values on the acting scope.
    #[test]
    fn outputs_project_as_json_strings() {
        let outputs = Vars::new().with("k1", "v1").with("n", 2);
        let writes = outputs_as_writes(&outputs);
        assert_eq!(writes.len(), 2);
        assert!(writes.contains(&("k1".to_string(), "\"v1\"".to_string())));
        assert!(writes.contains(&("n".to_string(), "2".to_string())));
    }
}
