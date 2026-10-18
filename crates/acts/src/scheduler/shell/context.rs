use crate::{
    ActError, Executor, MissingParamAction, Result,
    scheduler::{
        Runtime,
        shell::{Scope, ScopeTask},
    },
    snapshot::{SnapshotPolicy, SnapshotStore, join_scope, resolve_scope_params},
    utils::consts,
};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

tokio::task_local! {
    static CONTEXT: Context;
}

pub struct Context {
    pub runtime: Arc<Runtime>,
    pub executor: Arc<Executor>,
    pub proc: Arc<Scope>,
    task: RwLock<Arc<ScopeTask>>,
}

impl Clone for Context {
    fn clone(&self) -> Self {
        Context {
            runtime: self.runtime.clone(),
            executor: self.executor.clone(),
            proc: self.proc.clone(),
            task: RwLock::new(self.task.read().clone()),
        }
    }
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("pid", &self.proc.id())
            .field("tid", &self.task().id)
            .finish()
    }
}

impl Context {
    fn init_vars(&self, task: &Arc<ScopeTask>) {
        let inputs = task.inputs(self);
        debug!(inputs = %inputs, "init vars");

        // set the inputs to task's data
        self.task().set_data_with(|data| {
            for (k, v) in inputs.iter() {
                data.set(k, v.clone());
            }
        });
    }

    /// The headless context: the scope and task the effect/predicate seams
    /// built from a [`ScopeSpec`](super::scope::ScopeSpec).
    pub(crate) fn headless(runtime: Arc<Runtime>, proc: Arc<Scope>, task: Arc<ScopeTask>) -> Self {
        Self::from_scope(runtime, proc, task)
    }

    /// The context over already-built carriers.
    pub(crate) fn from_scope(
        runtime: Arc<Runtime>,
        proc: Arc<Scope>,
        task: Arc<ScopeTask>,
    ) -> Self {
        Context {
            executor: Arc::new(Executor::engine(&runtime)),
            runtime,
            proc,
            task: RwLock::new(task),
        }
    }

    pub fn scope<T, F: Fn() -> T>(ctx: &Context, f: F) -> T {
        // This is only an "is a scheduler context active?" probe; cloning the
        // context here would copy all scoped vars on every nested evaluation.
        if CONTEXT.try_with(|_| ()).is_ok() {
            f()
        } else {
            CONTEXT.sync_scope(ctx.clone(), f)
        }
    }

    /// Access the active scheduler context without cloning it.
    pub fn try_with_current<T, F: FnOnce(&Context) -> T>(f: F) -> Result<T> {
        CONTEXT
            .try_with(f)
            .map_err(|e| ActError::Runtime(e.to_string()))
    }

    pub fn with<T, F: Fn(&Context) -> T>(f: F) -> T {
        CONTEXT.with(|ctx| f(ctx))
    }

    pub fn task(&self) -> Arc<ScopeTask> {
        self.task.read().clone()
    }

    /// The stable idempotency key of the effect this call is executing, when
    /// the call is a package effect (A1c).
    ///
    /// The key is derived from the durable operation identity and is identical
    /// on every retry, replan and restart, so a package can forward it to an
    /// external system as its idempotency token. It is engine-owned and
    /// private: it is not a workflow variable and cannot be forged through
    /// `$env`, nor does it become part of the run's durable data.
    pub fn effect_key(&self) -> Option<String> {
        self.proc
            .with_env(|env| env.get::<String>(consts::EFFECT_KEY))
    }

    /// The directory this process's filesystem access is confined to
    /// (`<workdir>/<pid>`), or `None` when the engine config declares no
    /// workdir root. Packages that touch the filesystem (shell,
    /// and any custom one) read it here and refuse to leave it; a workflow
    /// script reads the same directory as `$env.WORK_DIR` (see
    /// [`crate::utils::consts::ENV_WORK_DIR`]).
    pub fn workdir(&self) -> Option<std::path::PathBuf> {
        self.proc.workdir()
    }

    /// A token that fires once this act should stop the work it started: the
    /// engine is shutting down, or the task was overridden while it ran (an
    /// `abort`/`cancel`/`skip`/`remove`/`next` action, or an error — see
    /// [`Task::set_state`](crate::Task::set_state)).
    ///
    /// A package that starts something outside the engine — a child process,
    /// an HTTP request, a subscription — selects on it and gives that work up
    /// when it fires, so a cancelled act does not hold its scheduler lane
    /// until its own deadline. It is a "stop now" signal, not a bound on how
    /// long an act may run: that is the act's own timeout.
    ///
    /// ```no_run
    /// # use acts::{ActError, Context, Result};
    /// # async fn wait(ctx: &Context) -> Result<()> {
    /// let cancel = ctx.cancellation_token();
    /// tokio::select! {
    ///     response = some_request() => Ok(response),
    ///     _ = cancel.cancelled() => Err(ActError::Runtime("cancelled".to_string())),
    /// }
    /// # }
    /// # async fn some_request() -> () {}
    /// ```
    pub fn cancellation_token(&self) -> CancellationToken {
        self.task().cancellation_token()
    }

    /// Seed this evaluation's inputs and resolve snapshot-backed sealed data.
    ///
    /// Both writes land in the headless scope only; nothing here is part of
    /// the run's durable state.
    pub async fn prepare(&self) -> Result<()> {
        self.init_vars(&self.task());
        self.resolve_sealed().await?;
        Ok(())
    }

    /// Resolve snapshot-backed sealed data for this effect prepare.
    ///
    /// The result lives only in the headless [`ScopeTask`]. It is never
    /// written to the aggregate or the `vars` row, and a restart/retry
    /// re-resolves from the in-memory snapshot cache instead of restoring an
    /// earlier seal. See the `snapshot` module docs for the boundary.
    pub async fn resolve_sealed(&self) -> Result<()> {
        // Snapshot-backed sealed data — a local cache read only. Feeds
        // (gRPC/NATS/Kafka adapters or embedders calling
        // `engine.snapshot().upsert`) write out-of-band, so this never does
        // network I/O. Missing params / absent data follow the target's
        // `on_missing` action.
        //
        // The process's owner credential decides which scopes it may read: a
        // process started by one caller cannot seal another subject's value,
        // however its vars were set. Enforced here rather than at the start
        // operation because the scope key is only resolvable on the task
        // chain, at prepare time.
        let owner = self.proc.owner_scope();
        let snapshots: Vec<(String, Arc<SnapshotStore>)> = self.runtime.snapshot_registry().list();
        for (name, store) in snapshots {
            let task = self.task();
            let options = &store.options;
            // A value already resolved on this headless task wins: one
            // prepare never re-seals a target it sealed itself. The kernel
            // builds a fresh scope per effect, so `PerProc` has no durable
            // lineage to inherit from — see `snapshot`'s module docs.
            if options.policy == SnapshotPolicy::PerProc && task.sealed(&self.proc, &name).is_some()
            {
                continue;
            }
            if task.has_sealed_local(&name) {
                continue;
            }
            let values = match resolve_scope_params(&task, &self.proc, options) {
                Ok(values) => values,
                Err(missing) => match options.on_missing {
                    MissingParamAction::Skip => continue,
                    MissingParamAction::Error => {
                        return Err(ActError::Runtime(format!(
                            "snapshot '{name}' missing required params: {missing:?}"
                        )));
                    }
                },
            };
            let scope = join_scope(&values);
            if !owner.allows(&name, &scope) {
                return Err(ActError::Denied(format!(
                    "snapshot '{name}' scope '{scope}' is not owned by subject '{}'",
                    owner.subject
                )));
            }
            match store.get(&scope) {
                Some(entry) => task.set_sealed(&name, entry.data.clone()),
                None => match options.on_missing {
                    MissingParamAction::Skip => continue,
                    MissingParamAction::Error => {
                        return Err(ActError::Runtime(format!(
                            "snapshot '{name}' has no data for scope '{scope}'"
                        )));
                    }
                },
            }
        }
        Ok(())
    }

    pub fn set_env<T>(&self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        // The workdir is engine-owned (it is what `$env.WORK_DIR` reads):
        // answering it from a stored value would let a script redefine where
        // its run is. A write is dropped, like the private keys in `$env`.
        if name == consts::ENV_WORK_DIR {
            return;
        }
        // in context, the global env is not writable
        // just set the value to local env of the process
        self.proc.with_env_mut(|data| {
            data.set(name, value);
        });
    }

    pub fn get_env<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        // The directory the process runs in has a readable name of its own
        // (`$env.WORK_DIR`), answered from the process itself — never from a
        // stored var or the OS environment of the same name, either of which
        // would name a directory the run is not in.
        if name == consts::ENV_WORK_DIR {
            let dir = self.workdir()?;
            return T::deserialize(serde_json::json!(dir.display().to_string())).ok();
        }
        // find the env from proc
        if let Some(v) = self.proc.with_env(|vars| vars.get(name)) {
            return Some(v);
        }

        // get from system env
        if let Ok(v) = std::env::var(name) {
            match T::deserialize(serde_json::json!(v)) {
                Ok(value) => return Some(value),
                Err(err) => {
                    tracing::warn!(
                        env = name,
                        target_type = std::any::type_name::<T>(),
                        error = %err,
                        "cannot convert system environment variable; treating it as absent"
                    );
                }
            }
        }

        None
    }

    pub fn eval<T: DeserializeOwned + Serialize>(&self, expr: &str) -> Result<T> {
        Context::scope(self, || self.runtime.env().eval::<T>(expr))
    }
}
