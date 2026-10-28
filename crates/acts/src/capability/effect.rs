//! The effect port: running a package's work inside the host's operation.
//!
//! The port is the kernel's capability; the shell's `PackageEffectRunner` is
//! one implementation. The scheduler holds the trait, never the implementation.

use crate::{
    ActRunAs, Result, Vars,
    scheduler::{DeferredAction, EffectKey, ScopeSpec},
};

/// One package effect request: everything the runner needs besides the
/// engine handles it was built with.
pub(crate) struct EffectRequest {
    /// The variable space the headless scope sees (the acting execution's
    /// scope, the run's env and authority).
    pub(crate) scope: ScopeSpec,
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
#[derive(Clone, Debug)]
pub(crate) struct EffectOutcome {
    /// The package's returned outputs — the only way a package changes the
    /// run's variables. The host writes them to the acting scope.
    pub(crate) outputs: Option<Vars>,
    /// The client decision a package deferred to its own operation's commit
    /// (`Context::defer_action`, the `acts.core.action` effect): the host
    /// plans it exactly like a durable client decision, in that same commit.
    pub(crate) decision: Option<DeferredAction>,
}

impl EffectOutcome {
    pub(crate) fn empty() -> Self {
        Self {
            outputs: None,
            decision: None,
        }
    }

    /// A manually reconciled applied effect: the operator proved the external
    /// work happened, but the package's returned values were not captured.
    /// Only the declared outputs can be supplied.
    pub(crate) fn from_outputs(outputs: Vars) -> Self {
        Self {
            outputs: Some(outputs),
            decision: None,
        }
    }
}

/// What an executor can prove about one effect key (A1c).
///
/// The observation is the executor's own durable fact, not the aggregate's:
/// the host asks it when an `EffectUnknown` operation is recovered and only
/// lets the proof decide the operation's fate. `Unknown` (the default for an
/// executor that does not keep observations) is always safe — it leaves the
/// operation for manual reconciliation.
// `Failed`/`NotApplied` are the executor-facing vocabulary: an attached
// runner (in-tree or an embedder's) is the only constructor, and the default
// `observe` answers `Unknown`. The host consumes all four variants, so the
// lint would flag a vocabulary that is used by construction.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum EffectObservation {
    /// The effect ran and produced this outcome; recovery re-plans it without
    /// running it again.
    Applied(EffectOutcome),
    /// The effect ran and failed with this message; recovery takes the
    /// failure path without running it again.
    Failed(String),
    /// The executor can prove the effect did not run.
    NotApplied,
    /// The executor cannot prove either way.
    Unknown,
}

/// Runs one package effect. The host's serving assembly installs an
/// implementation; without one the host auto-completes (the shadow contract).
#[async_trait::async_trait]
pub(crate) trait EffectRunner: Send + Sync {
    /// Resolve the step's package and validate its params — the legacy act's
    /// `run()` before the `run_as` match. Every `run_as` goes through here at
    /// task start, exactly like the legacy engine: a schema violation fails
    /// the act whether it runs a package or waits for a client.
    async fn resolve(&self, request: &EffectRequest) -> Result<ResolvedEffect>;

    /// The package's work: a Func package executes (an Irq/Msg package has
    /// none). The outcome carries the values to fold into the operation's commit.
    ///
    /// `key` is the stable identity of this effect (see [`EffectKey`]); an
    /// executor that forwards it to an external system gets the same token on
    /// every retry, replan and restart. Reusing a key is the executor's
    /// deduplication contract, not a new one: the host never mints a second
    /// key for one operation.
    async fn run(&self, request: EffectRequest, key: EffectKey) -> Result<EffectOutcome>;

    /// Whether executing this effect twice under the same key is provably
    /// harmless. Only such an effect may be re-queued after an executor
    /// observation proves it did not run. The default is "not provable".
    fn idempotent(&self, _request: &EffectRequest) -> bool {
        false
    }

    /// Whether the package's work runs even though the act waits for an
    /// external answer (`auto == "0"`). A waiting act normally executes
    /// nothing — an `Irq` waits for its client — but a subflow's act must
    /// start the child before it waits for the child's return. The default
    /// is "no"; the package runner answers from the package's `run_as`.
    fn executes_while_waiting(&self, _package: &str) -> bool {
        false
    }

    /// The package's run-bound kind — the import-time `run_as` decision the
    /// auto and msg maps were built from. A pushed act is not in those maps
    /// (the model is immutable), so its push derives the decisions from here
    /// at push time. `None` for an unregistered package, the `Irq` default.
    fn run_as(&self, _package: &str) -> Option<ActRunAs> {
        None
    }

    /// The executor's own observation for `key`. A runner that does not keep
    /// one answers [`EffectObservation::Unknown`], which keeps the operation in
    /// manual reconciliation.
    async fn observe(&self, _key: &EffectKey) -> Result<EffectObservation> {
        Ok(EffectObservation::Unknown)
    }
}
