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

    /// Read one environment value. The process env is read-only: it is built
    /// from the run's declared `env`, the engine's own keys and the host
    /// environment, and nothing at run time can change it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::task_graph::TaskId;
    use crate::scheduler::shell::scope::ScopeSpec;
    use crate::store::MemoryStore;
    use crate::{Config, MissingParamAction, Vars, model::Workflow};

    fn new_runtime(config: &Config) -> Arc<Runtime> {
        Runtime::new(config, Some(Arc::new(MemoryStore::new()))).unwrap()
    }

    fn spec(owner: crate::ScopePolicy, vars: Vec<(String, String)>) -> ScopeSpec {
        ScopeSpec {
            process_id: "p-ctx".into(),
            node: "s1".into(),
            task_id: TaskId::new("s1", 0).unwrap(),
            model: Workflow::new()
                .with_id("wf-ctx")
                .with_var("v", 1)
                .with_env("declared", "from-env")
                .with_step(|step| {
                    step.with_id("s1")
                        .with_uses("test.irq", Vars::new().with("p", "${{ v }}"))
                }),
            vars,
            env: vec![("declared".into(), serde_json::json!("from-env"))],
            owner,
        }
    }

    fn ctx(runtime: &Arc<Runtime>, spec: &ScopeSpec) -> Context {
        crate::scheduler::shell::scope::headless_scope(runtime, spec)
            .unwrap()
            .2
    }

    fn open_owner() -> crate::ScopePolicy {
        crate::ScopePolicy {
            subject: "u1".into(),
            all: true,
            ..crate::ScopePolicy::default()
        }
    }

    /// The debug rendering names the run and the execution, the task-local
    /// scope can be entered from outside or reused inside, and the engine
    /// handles are the scope's and the runtime's.
    #[tokio::test]
    async fn the_context_renders_scopes_with_and_without_a_running_scope() {
        let runtime = new_runtime(&Config::default());
        let ctx = ctx(
            &runtime,
            &spec(open_owner(), vec![("v".into(), "1".into())]),
        );
        let text = format!("{ctx:?}");
        assert!(text.contains("Context"), "{text}");
        assert!(text.contains("p-ctx"), "{text}");

        // Outside: the scope is installed for the call and removed after.
        let pid = Context::scope(&ctx, || ctx.proc.id().to_string());
        assert_eq!(pid, "p-ctx");
        assert!(Context::try_with_current(|_| ()).is_err());

        // Inside: the installed scope is reused, never re-installed.
        let seen = Context::scope(&ctx, || {
            Context::try_with_current(|inner| inner.proc.id().to_string()).unwrap()
        });
        assert_eq!(seen, "p-ctx");

        // Clone, task handle and the cancellation child all point at the run.
        let clone = ctx.clone();
        assert_eq!(clone.task().id, ctx.task().id);
        assert!(!ctx.cancellation_token().is_cancelled());
    }

    /// The env surface: the run directory has its own name, the run env is
    /// read first, the process env answers a scalar, a type mismatch reads as
    /// absent, and a missing name is `None`.
    #[tokio::test]
    async fn the_context_answers_env_from_the_run_and_the_process() {
        let mut config = Config::default();
        config.data.workdir = Some(
            std::env::temp_dir()
                .join(format!("acts-ctx-env-{}", crate::utils::longid()))
                .display()
                .to_string(),
        );
        let runtime = new_runtime(&config);
        let ctx = ctx(
            &runtime,
            &spec(open_owner(), vec![("v".into(), "1".into())]),
        );

        let dir: String = ctx.get_env(consts::ENV_WORK_DIR).unwrap();
        assert!(dir.contains("p-ctx"), "{dir}");
        assert_eq!(
            ctx.get_env::<String>("declared").as_deref(),
            Some("from-env")
        );
        assert_eq!(ctx.eval::<i32>("v").unwrap(), 1);
        assert!(ctx.workdir().is_some());

        // A process env variable that cannot convert to the requested type is
        // treated as absent instead of failing the effect.
        assert!(ctx.get_env::<i32>("PATH").is_none());
        assert!(ctx.get_env::<String>("ACTS_SURELY_MISSING_KEY").is_none());
    }

    /// A registered snapshot target seals its value onto the headless task;
    /// the value never reaches `find`.
    #[tokio::test]
    async fn a_snapshot_target_seals_its_data() {
        let runtime = new_runtime(&Config::default());
        let options = crate::snapshot::SnapshotOptions {
            scope: vec!["uid".into()],
            on_missing: MissingParamAction::Error,
            ..crate::snapshot::SnapshotOptions::per_task()
        };
        runtime.register_snapshot("profile", options).unwrap();
        runtime.snapshot_store("profile").unwrap().upsert(
            "u9",
            1,
            Vars::new().with("name", "sealed"),
        );

        let ctx = ctx(
            &runtime,
            &spec(open_owner(), vec![("uid".into(), "\"u9\"".into())]),
        );
        ctx.prepare().await.unwrap();
        let sealed = ctx.task().sealed(&ctx.proc, "profile").unwrap();
        assert_eq!(sealed.get::<String>("name").as_deref(), Some("sealed"));
        assert!(ctx.task().find::<Vars>(&ctx.proc, "profile").is_none());

        // A second prepare keeps the local seal instead of re-resolving.
        ctx.prepare().await.unwrap();
        assert!(ctx.task().has_sealed_local("profile"));
    }

    /// A missing scope parameter follows the target's `on_missing`: skip the
    /// target, or refuse the prepare with the missing names.
    #[tokio::test]
    async fn a_missing_scope_parameter_follows_on_missing() {
        let runtime = new_runtime(&Config::default());
        let mut skip = crate::snapshot::SnapshotOptions::per_task();
        skip.scope = vec!["uid".into()];
        skip.on_missing = MissingParamAction::Skip;
        runtime.register_snapshot("skip", skip).unwrap();
        let ctx = ctx(&runtime, &spec(open_owner(), Vec::new()));
        ctx.prepare().await.unwrap();
        assert!(!ctx.task().has_sealed_local("skip"));

        let mut error = crate::snapshot::SnapshotOptions::per_task();
        error.scope = vec!["uid".into()];
        error.on_missing = MissingParamAction::Error;
        runtime.register_snapshot("error", error).unwrap();
        let failure = ctx.prepare().await.unwrap_err();
        assert!(
            format!("{failure}").contains("missing required params"),
            "{failure}"
        );
    }

    /// A scope the caller does not own is denied before the store is read,
    /// and absent data follows `on_missing` like an absent parameter.
    #[tokio::test]
    async fn snapshot_authority_and_absent_data_are_enforced() {
        let runtime = new_runtime(&Config::default());
        let options = crate::snapshot::SnapshotOptions {
            scope: vec!["uid".into()],
            on_missing: MissingParamAction::Error,
            ..crate::snapshot::SnapshotOptions::per_task()
        };
        runtime.register_snapshot("profile", options).unwrap();
        runtime.snapshot_store("profile").unwrap().upsert(
            "u9",
            1,
            Vars::new().with("name", "sealed"),
        );

        // deny_all: the subject may read nothing, whatever the scope key.
        let denied = ctx(
            &runtime,
            &spec(
                crate::ScopePolicy::deny_all(),
                vec![("uid".into(), "\"u9\"".into())],
            ),
        );
        let failure = denied.prepare().await.unwrap_err();
        assert!(matches!(failure, ActError::Denied(_)), "{failure:?}");

        // Owned, but the store has no entry for the scope: Error refuses.
        let missing = ctx(
            &runtime,
            &spec(open_owner(), vec![("uid".into(), "\"u-missing\"".into())]),
        );
        let failure = missing.prepare().await.unwrap_err();
        assert!(
            format!("{failure}").contains("has no data for scope"),
            "{failure}"
        );

        // ...and Skip leaves the target unsealed (a fresh registry, so the
        // Error target above does not decide this case).
        let runtime = new_runtime(&Config::default());
        let mut skip = crate::snapshot::SnapshotOptions {
            scope: vec!["uid".into()],
            ..crate::snapshot::SnapshotOptions::per_task()
        };
        skip.on_missing = MissingParamAction::Skip;
        runtime.register_snapshot("skip", skip).unwrap();
        let missing = ctx(
            &runtime,
            &spec(open_owner(), vec![("uid".into(), "\"u-missing\"".into())]),
        );
        missing.prepare().await.unwrap();
        assert!(!missing.task().has_sealed_local("skip"));
    }

    /// A `PerProc` target already sealed on the evaluation's lineage is not
    /// re-resolved: the first value wins for this prepare.
    #[tokio::test]
    async fn a_per_proc_seal_is_resolved_once_per_lineage() {
        let runtime = new_runtime(&Config::default());
        runtime
            .register_snapshot("profile", crate::snapshot::SnapshotOptions::per_proc())
            .unwrap();
        runtime
            .snapshot_store("profile")
            .unwrap()
            .upsert("", 1, Vars::new().with("name", "first"));
        let ctx = ctx(&runtime, &spec(open_owner(), Vec::new()));
        ctx.prepare().await.unwrap();
        let first = ctx.task().sealed(&ctx.proc, "profile").unwrap();
        assert_eq!(first.get::<String>("name").as_deref(), Some("first"));

        runtime.snapshot_store("profile").unwrap().upsert(
            "",
            2,
            Vars::new().with("name", "second"),
        );
        ctx.prepare().await.unwrap();
        let kept = ctx.task().sealed(&ctx.proc, "profile").unwrap();
        assert_eq!(
            kept.get::<String>("name").as_deref(),
            Some("first"),
            "the local seal is the preparation's value"
        );
    }
}
