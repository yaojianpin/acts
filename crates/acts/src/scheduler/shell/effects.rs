//! The effect seam: running a Func package's work inside the host's
//! `ApplyAction` operation.
//!
//! A host driving without an attached runner auto-completes (the bare
//! contract the kernel's own tests use); the serving host has
//! [`PackageEffectRunner`] attached, which executes the step's package for
//! real and folds its outputs into the same atomic commit that completes the
//! execution. The effect runs before the state commits, exactly like the
//! legacy `exec` → persist ordering, but it is **not** blindly re-run when
//! that commit is lost: A1a turns the operation `EffectUnknown`, and A1c
//! lets only the executor's observation decide it. The executor receives the
//! stable [`EffectKey`] of the operation, so a retry, replan or restart can
//! present the same idempotency token instead of a fresh random one.
//!
//! The runner drives the package through a *headless* scope: the acting
//! node's content, its data/sealed values and the run's env, with no
//! `Process` or node tree. Packages read the context they always read — the params
//! resolve through the same `fill_params` path — and nothing the headless
//! scope touches is durably written: outputs come back as values and land
//! in the aggregate through the operation's own plan.

use crate::{
    ActRunAs, Result, Vars,
    scheduler::{
        Runtime,
        core::model::effect::EffectKey,
        shell::{Context, Scope, ScopeTask},
    },
    utils::consts,
};
use std::sync::Arc;

// The port lives in `capability`; this module owns the shell's default
// implementation. The re-export keeps the implementation's tests and the
// shell's other modules addressing the vocabulary beside that implementation.
pub(crate) use crate::capability::{
    EffectObservation, EffectOutcome, EffectRequest, EffectRunner, ResolvedEffect,
};

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
    /// own directory (`<workdir>/<pid>`) and the workflow's declared env are
    /// seeded, so `$env.WORK_DIR` and `$env.X` answer read-only values.
    fn headless(&self, request: &EffectRequest) -> Result<(Arc<Scope>, Arc<ScopeTask>, Context)> {
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
        let params = match self.runtime.package().get(definition.id()) {
            Some(_) => {
                let package = self
                    .runtime
                    .package()
                    .create(definition.id(), self.runtime.config())?;
                package.prepare_params(&ctx, params).await?
            }
            // Published metadata without a runtime instance: nothing can
            // normalize the params, so the request carries the declared ones.
            None => params,
        };
        Ok(ResolvedEffect {
            params,
            options: serde_json::to_value(task.options()).unwrap_or(serde_json::Value::Null),
        })
    }

    async fn run(&self, request: EffectRequest, key: EffectKey) -> Result<EffectOutcome> {
        let definition = self.runtime.package_definition(&request.package).await?;
        // Every `run_as` prepares the act's scope first — the legacy act's own
        // `run()`: seed its vars and seal the snapshot data its target
        // declares, under the run's authority. A resolution failure (a denied
        // scope, a missing required param under `on_missing = error`) fails
        // the act *here*, which is the operation the start operation deferred it to; the
        // legacy engine prepared before it dispatched on `run_as` too, so an
        // Irq act whose seal is refused failed instead of waiting forever.
        let (process, task, ctx) = self.headless(&request)?;
        // The stable effect identity travels into the package's own process
        // env under a private key: `Context::effect_key` answers it and the
        // read-only `$env` never exposes a private key, so it never becomes
        // durable run data.
        process.with_env_mut(|env| env.set(consts::EFFECT_KEY, key.wire()));
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
        let params = package.prepare_params(&ctx, params).await?;
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

        // The write-back: only the outputs the package returned. They fold
        // into the operation's atomic commit, so a crash before the commit
        // re-runs the package with the same pre-state (the legacy `exec` →
        // persist ordering). A decision the package deferred on this context
        // rides the same commit (see `EffectOutcome::decision`).
        Ok(EffectOutcome {
            outputs,
            decision: ctx.take_deferred_action(),
        })
    }

    fn idempotent(&self, request: &EffectRequest) -> bool {
        // The declaration is the package registration's, next to its
        // `run_as`: a package opts in only when executing it twice with the
        // same key is provably harmless.
        self.runtime
            .package()
            .get(&request.package)
            .is_some_and(|register| (register.idempotent)())
    }

    fn executes_while_waiting(&self, package: &str) -> bool {
        // A `Func` package is the kernel's executing kind. Only a waiting
        // `Func` package (the subflow) reaches this question: the start
        // operation runs the effect and leaves the act waiting for the return
        // that completes it.
        self.runtime
            .package()
            .get(package)
            .is_some_and(|register| (register.meta)().run_as == ActRunAs::Func)
    }

    fn run_as(&self, package: &str) -> Option<crate::ActRunAs> {
        self.runtime
            .package()
            .get(package)
            .map(|register| (register.meta)().run_as)
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

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use crate::scheduler::core::model::operation::OperationId;
    use crate::scheduler::core::model::task_graph::TaskId;
    use crate::store::MemoryStore;
    use crate::{Config, model::Workflow};

    fn runtime() -> Arc<Runtime> {
        Runtime::new(&Config::default(), Some(Arc::new(MemoryStore::new()))).unwrap()
    }

    fn request(package: &str) -> EffectRequest {
        EffectRequest {
            scope: super::super::scope::ScopeSpec {
                process_id: "p-effect".into(),
                node: "s1".into(),
                task_id: TaskId::new("s1", 0).unwrap(),
                model: Workflow::new().with_step(|step| step.with_id("s1")),
                pushed: None,
                vars: Vec::new(),
                snapshot: None,
                env: Vec::new(),
                owner: crate::ScopePolicy::deny_all(),
            },
            package: package.into(),
        }
    }

    /// The declared idempotency is read from the registration, and an
    /// unregistered package is not provably idempotent.
    #[tokio::test]
    async fn the_package_idempotency_is_the_registration_declaration() {
        let runner = PackageEffectRunner::new(runtime());
        runner.runtime.package().register(
            "idem",
            &crate::package::ActPackageRegister {
                meta: || panic!("unused"),
                create: |_| panic!("unused"),
                idempotent: || true,
            },
        );
        runner.runtime.package().register(
            "plain",
            &crate::package::ActPackageRegister {
                meta: || panic!("unused"),
                create: |_| panic!("unused"),
                idempotent: || false,
            },
        );
        assert!(runner.idempotent(&request("idem")));
        assert!(!runner.idempotent(&request("plain")));
        assert!(!runner.idempotent(&request("missing")));
    }

    /// A runner that declares nothing is not idempotent and cannot observe
    /// anything: both defaults keep the operation safe.
    #[tokio::test]
    async fn the_runner_defaults_are_conservative() {
        struct Bare;

        #[async_trait::async_trait]
        impl EffectRunner for Bare {
            async fn resolve(&self, _request: &EffectRequest) -> Result<ResolvedEffect> {
                Ok(ResolvedEffect {
                    params: serde_json::Value::Null,
                    options: serde_json::Value::Null,
                })
            }

            async fn run(&self, _request: EffectRequest, _key: EffectKey) -> Result<EffectOutcome> {
                Ok(EffectOutcome::empty())
            }
        }

        let request = request("any");
        assert!(!Bare.idempotent(&request));
        let operation = OperationId {
            process_id: "p".into(),
            task: TaskId::new("s1", 0).unwrap(),
            sequence: 0,
        };
        assert!(matches!(
            Bare.observe(&EffectKey::of(&operation)).await.unwrap(),
            EffectObservation::Unknown
        ));
    }
}
