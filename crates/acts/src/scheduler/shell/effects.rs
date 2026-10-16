//! The effect seam: running a Func package's work inside the host's
//! `ApplyAction` operation.
//!
//! A host driving without an attached runner auto-completes (the bare
//! contract the kernel's own tests use); the serving host has
//! [`PackageEffectRunner`] attached, which executes the step's package for
//! real and folds its outputs into the same atomic commit that completes the
//! execution. The contract is the legacy
//! engine's own: the effect runs before the state commits (at-least-once —
//! a crash between effect and commit re-runs the package on recovery,
//! exactly like the legacy `exec` → persist ordering).
//!
//! The runner drives the package through a *headless* scope: a bare
//! in-memory [`Process`] over the declaring model with one task on the
//! acting node. Packages read the context they always read — the params
//! resolve through the same `fill_params` path — and nothing the headless
//! scope touches is durably written: outputs come back as values and land
//! in the aggregate through the operation's own plan.

use crate::{
    ActRunAs, Result, Vars,
    scheduler::{
        Runtime,
        shell::{Context, Process, Task},
    },
};
use std::sync::Arc;

/// One package effect request: everything the runner needs besides the
/// engine handles it was built with.
pub(crate) struct EffectRequest {
    /// The variable space the headless scope sees (the acting execution's
    /// scope, the run's env and authority).
    pub(crate) scope: super::scope::ScopeSpec,
    /// The step's `uses` package id.
    pub(crate) package: String,
}

/// The act facts one `uses` step resolves to before it runs — the legacy
/// act's `run()` up to (not including) the package's own execution. The
/// message operation needs them for every `run_as` (the legacy act's Created
/// message carries the resolved params and its options); the effect operation needs
/// them for the package it executes (Func).
pub(crate) struct ResolvedEffect {
    /// The step's resolved params (`task.params(ctx)`), the message's
    /// `params` input.
    pub(crate) params: serde_json::Value,
    /// The step's declared options, the message's `options` input.
    pub(crate) options: serde_json::Value,
}

/// What one package effect left behind: the values the host folds into the
/// same atomic commit that completes the execution.
pub(crate) struct EffectOutcome {
    /// The package's returned outputs — the legacy
    /// `task.update_data(&vars)`, written to the acting scope.
    pub(crate) outputs: Option<Vars>,
    /// The headless task's own data *beyond what the seam seeded* — a package
    /// that wrote task data directly. Only the delta: echoing the seeded
    /// scope back would shadow an ancestor's variable with a local copy.
    pub(crate) data: Vec<(String, serde_json::Value)>,
    /// The process env the package wrote through `Context::set_env`. Folds
    /// into the run's durable env map, so every later task of the run reads
    /// it as `$env.X`.
    pub(crate) env: Vec<(String, serde_json::Value)>,
}

/// Runs one package effect. Attach with
/// [`KernelHost::attach_effect_runner`](super::KernelHost::attach_effect_runner);
/// without one the host auto-completes (the shadow contract).
#[async_trait::async_trait]
pub(crate) trait EffectRunner: Send + Sync {
    /// Resolve the step's package and validate its params — the legacy act's
    /// `run()` before the `run_as` match. Every `run_as` goes through here at
    /// task start, exactly like the legacy engine: a schema violation fails
    /// the act whether it runs a package or waits for a client.
    async fn resolve(&self, request: &EffectRequest) -> Result<ResolvedEffect>;

    /// The package's work: a Func package executes (an Irq/Msg package has
    /// none). The outcome carries the values to fold into the operation's commit.
    async fn run(&self, request: EffectRequest) -> Result<EffectOutcome>;
}

/// Runs real packages through the engine's registry and a headless scope.
pub(crate) struct PackageEffectRunner {
    runtime: Arc<Runtime>,
}

impl PackageEffectRunner {
    pub(crate) fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }

    /// The headless scope the legacy run evaluated in: a bare in-memory
    /// process over the declaring model with one task on the acting node (see
    /// [`super::scope`]). The run's caller authority travels with it, so the
    /// seal below reads the scopes its starter owns and no others; the run's
    /// own directory (`<workdir>/<pid>`) and the durable env map are seeded,
    /// so `$env.WORK_DIR` and `$env.X` answer exactly what the legacy process
    /// env answered.
    fn headless(&self, request: &EffectRequest) -> Result<(Arc<Process>, Arc<Task>, Context)> {
        super::scope::headless_scope(&self.runtime, &request.scope)
    }
}

#[async_trait::async_trait]
impl EffectRunner for PackageEffectRunner {
    async fn resolve(&self, request: &EffectRequest) -> Result<ResolvedEffect> {
        let definition = self.runtime.package_definition(&request.package).await?;
        // The legacy task init path: seed the task's own vars and seal the
        // snapshot data its target declares, under the owner's authority. A
        // seal error — a denied scope, a missing required param under
        // `on_missing = error` — fails the effect, exactly where the legacy
        // run would fail the task.
        let (_, task, ctx) = self.headless(request)?;
        ctx.prepare().await?;
        let params = task.params(&ctx);
        definition.validate(&params)?;
        Ok(ResolvedEffect {
            params,
            options: serde_json::to_value(task.options()).unwrap_or(serde_json::Value::Null),
        })
    }

    async fn run(&self, request: EffectRequest) -> Result<EffectOutcome> {
        let definition = self.runtime.package_definition(&request.package).await?;
        // Every `run_as` prepares the act's scope first — the legacy act's own
        // `run()`: seed its vars and seal the snapshot data its target
        // declares, under the run's authority. A resolution failure (a denied
        // scope, a missing required param under `on_missing = error`) fails
        // the act *here*, which is the operation the start operation deferred it to; the
        // legacy engine prepared before it dispatched on `run_as` too, so an
        // Irq act whose seal is refused failed instead of waiting forever.
        let (process, task, ctx) = self.headless(&request)?;
        ctx.prepare().await?;
        if definition.run_as() != ActRunAs::Func {
            // The package itself only executes for Func: Irq waits for the
            // client and Msg only emits, so neither folds anything into the
            // completion.
            return Ok(EffectOutcome::empty());
        }

        // Params resolve through the same fill path the legacy run uses.
        let params = task.params(&ctx);
        definition.validate(&params)?;

        let package = self
            .runtime
            .package()
            .create(definition.id(), self.runtime.config())?;
        // A package that panics is a failed act, not a dead engine: the panic
        // is contained here and reported through the ordinary package-error
        // path (the execution `Failed`, its error walk carries it), exactly
        // where the legacy engine reported a panicking `run`.
        let outputs = match super::errors::CatchPanic::new(package.execute(&ctx, &params)).await {
            Ok(outputs) => outputs?,
            Err(payload) => {
                return Err(crate::ActError::Package(format!(
                    "package '{}' panicked: {}",
                    definition.id(),
                    super::errors::panic_message(&payload)
                )));
            }
        };

        // The write-back: what the package left in the headless scope beyond
        // what the seam seeded — its own task data and the process env — and
        // the outputs it returned. All of it folds into the operation's atomic
        // commit, so a crash before the commit re-runs the package with the
        // same pre-state (the legacy `exec` → persist ordering).
        let env = non_private(process.env());
        Ok(EffectOutcome {
            outputs,
            data: data_delta(&task, &request.scope.vars),
            env,
        })
    }
}

impl EffectOutcome {
    pub(crate) fn empty() -> Self {
        Self {
            outputs: None,
            data: Vec::new(),
            env: Vec::new(),
        }
    }
}

/// The non-private entries of one var map, in iteration order.
fn non_private(vars: Vars) -> Vec<(String, serde_json::Value)> {
    vars.iter()
        .filter(|(name, _)| !crate::utils::consts::is_private_key(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The headless task data beyond the seeded scope: a key the seam did not
/// seed, or one whose value the package changed. Echoing an unchanged seeded
/// value back would shadow an ancestor's variable with a local copy.
fn data_delta(task: &Arc<Task>, seeded: &[(String, String)]) -> Vec<(String, serde_json::Value)> {
    task.data()
        .iter()
        .filter(|(name, _)| !crate::utils::consts::is_private_key(name))
        .filter(|(name, value)| {
            !seeded.iter().any(|(seeded_name, raw)| {
                seeded_name.as_str() == name.as_str()
                    && serde_json::from_str::<serde_json::Value>(raw).ok().as_ref() == Some(value)
            })
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
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
