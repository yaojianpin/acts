use crate::snapshot::SnapshotOptions;
use crate::{
    AccessControl, ActPackage, ActPlugin, Config, Engine, config::ConfigLog,
    package::ActPackageRegister, scheduler::Runtime, store::KvStore,
};
use std::{path::Path, sync::Arc};
use tracing::{info, warn};

pub struct EngineBuilder {
    config: Config,
    plugins: Vec<Arc<dyn ActPlugin>>,
    packages: Vec<ActPackageRegister>,
    snapshots: Vec<(String, SnapshotOptions)>,
    store: Option<Arc<dyn KvStore>>,
    acl: Option<Arc<dyn AccessControl>>,
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineBuilder {
    pub fn new() -> Self {
        let mut config = Config::default();
        #[cfg(not(test))]
        let file = Path::new("config/acts.toml");

        #[cfg(test)]
        let file = Path::new("test/acts.toml");

        if file.exists() {
            match Config::create(file) {
                Ok(loaded) => config = loaded,
                Err(err) => {
                    warn!(error = %err, path = %file.display(), "failed to load default config; using default engine config")
                }
            }
        }

        Self {
            config,
            plugins: Vec::new(),
            packages: Vec::new(),
            snapshots: Vec::new(),
            store: None,
            acl: None,
        }
    }

    pub fn set_config(mut self, config: &Config) -> Self {
        self.config = config.clone();
        self
    }

    pub fn config(&self) -> Config {
        self.config.clone()
    }

    pub fn set_config_source(mut self, source: &Path) -> crate::Result<Self> {
        self.config = Config::create(source)?;
        Ok(self)
    }

    /// Install the engine's access control — the user registry and session
    /// store. The shipped implementation is `acts_acl::UserAcl`, whose
    /// `AclUsers` trait adds the one-call form to this builder:
    ///
    /// ```text
    /// use acts::Engine;
    /// use acts_acl::AclUsers;
    ///
    /// let engine = Engine::builder().with_user_acl().start();
    /// ```
    ///
    /// (A crate outside this one cannot dev-depend on `acts-acl` without
    /// forming a publish ring, so that example is not a doctest here.)
    ///
    /// A bare engine runs [`AnonymousAcl`](crate::AnonymousAcl): no users, no
    /// login, the anonymous catalogue-only policy for every caller.
    pub fn set_acl(mut self, acl: Arc<dyn AccessControl>) -> Self {
        self.acl = Some(acl);
        self
    }

    /// Turn access control off: every caller resolves to
    /// [`Principal::unrestricted`](crate::Principal::unrestricted) and nothing
    /// is checked.
    ///
    /// This is the explicit opt-out, and the setting a test or a local demo
    /// uses — it is spelled out at the call site, never inferred. There is no
    /// implicit version of it: an engine without this call runs whatever
    /// access control was installed ([`AnonymousAcl`](crate::AnonymousAcl)
    /// when none was — the anonymous catalogue-only policy).
    pub fn disable_acl(mut self) -> Self {
        self.acl = Some(Arc::new(crate::DisabledAcl));
        self
    }

    pub fn log(mut self, dir: &str, level: &str) -> Self {
        self.config_mut().data.log = Some(ConfigLog {
            dir: dir.to_string(),
            level: level.to_string(),
            max_files: None,
        });
        self
    }

    /// Bound the runs in flight at once ([`Config::cache_cap`]): a start
    /// beyond it parks, and a run that settles admits the parked run that has
    /// waited longest.
    pub fn cache_size(mut self, size: i64) -> Self {
        self.config_mut().data.cache_cap = Some(size);
        self
    }

    pub fn tick_interval_secs(mut self, secs: i64) -> Self {
        self.config_mut().data.tick_interval_secs = Some(secs);
        self
    }

    /// Set the filesystem root for process directories: every process runs in
    /// its own `<workdir>/<pid>`, which is what `Scope::workdir` and
    /// `Context::workdir` answer and what `$env.WORK_DIR` names. An empty
    /// path is refused at `start` (remove the setting to run without
    /// directory control).
    pub fn workdir(mut self, workdir: &str) -> Self {
        self.config_mut().data.workdir = Some(workdir.to_string());
        self
    }

    /// Set how often an unacknowledged message delivery is re-sent before it
    /// turns into an `Error` that needs manual handling (default 20). Must be
    /// at least 1 — `start` refuses a smaller value instead of running with
    /// the retry budget switched off.
    pub fn max_message_retry_times(mut self, retry_times: i32) -> Self {
        self.config_mut().data.max_message_retry_times = Some(retry_times);
        self
    }
    /// bound the times a tree node can be executed in one process (protects
    /// against unbounded task creation caused by a node self-loop or a cyclic
    /// `next`; default 1000). Must be at least 1 — `start` refuses a smaller
    /// value instead of running with the guard switched off.
    pub fn max_node_run_times(mut self, times: i64) -> Self {
        self.config_mut().data.max_node_run_times = Some(times);
        self
    }

    /// Set the number of serial task lanes. Independent pids can execute on
    /// different lanes; tasks belonging to the same pid retain FIFO order.
    pub fn scheduler_workers(mut self, workers: usize) -> Self {
        self.config_mut().data.scheduler_workers = Some(workers);
        self
    }

    /// Set the maximum in-memory scheduler backlog. It is split across the task
    /// lanes, and a lane that is full overflows its producers to the durable
    /// outbox (`next` work spills; a fresh start fails).
    pub fn scheduler_queue_cap(mut self, cap: usize) -> Self {
        self.config_mut().data.scheduler_queue_cap = Some(cap);
        self
    }

    /// Set the number of concurrent store-writer shards. Independent pids are
    /// persisted concurrently, one consumer per shard; the ops of one pid
    /// always land in the same shard and keep their enqueue order.
    pub fn store_writer_workers(mut self, workers: usize) -> Self {
        self.config_mut().data.store_writer_workers = Some(workers);
        self
    }

    /// Set the maximum store-writer backlog. It is split across the shards; a
    /// shard with no room makes the writer refuse new work (`QueueFull`) until
    /// the backlog drains, while the bookkeeping of work already in flight
    /// waits for room.
    pub fn store_writer_queue_cap(mut self, cap: usize) -> Self {
        self.config_mut().data.store_writer_queue_cap = Some(cap);
        self
    }

    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ActPlugin, Message, Engine, Workflow, Result};
    ///
    /// #[derive(Clone)]
    /// struct TestPlugin;
    /// impl TestPlugin {
    ///     fn new() -> Self {
    ///         Self
    ///     }
    /// }
    /// #[async_trait::async_trait]
    /// impl ActPlugin for TestPlugin {
    ///     fn on_init(&self, engine: &Engine) -> Result<()> {
    ///         println!("TestPlugin");
    ///         engine.channel().on_start(|_| async {});
    ///         engine.channel().on_complete(|_| async {});
    ///         engine.channel().on_message(|_| async {});
    ///         Ok(())       
    ///     }
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::builder().add_plugin(&TestPlugin::new()).start().await.unwrap();
    /// }
    /// ```
    pub fn add_plugin<T>(mut self, plugin: &T) -> Self
    where
        T: ActPlugin + Clone + 'static,
    {
        self.plugins.push(Arc::new(plugin.clone()));
        self
    }

    /// register package
    //// ## Example
    /// ```no_run
    /// use acts::{ActPackage, ActPackageDefinition, ActPackageCatalog, Context, Engine, Result, Vars};   
    /// use serde::{Deserialize, Serialize};
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Clone, Deserialize, Serialize)]
    /// struct MyPackage;
    ///
    /// #[async_trait::async_trait]
    /// impl ActPackage for MyPackage {
    ///    fn definition() -> ActPackageDefinition {
    ///       ActPackageDefinition {
    ///         id: "my_package",
    ///         name: "my package",
    ///         desc: "",
    ///         icon: "",
    ///         doc: "",
    ///         version: "0.1.0",
    ///         schema: json!({}),
    ///         options: Some(json!({})),
    ///         run_as: acts::ActRunAs::Func,
    ///         resources: vec![],
    ///         catalog: ActPackageCatalog::App,
    ///       }
    ///     }  
    ///
    ///     fn new(_config: &acts::Config) -> Result<Self> {
    ///       Ok(Self)
    ///     }
    ///
    ///     async fn execute(&self, ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
    ///       // do something with ctx
    ///       Ok(None)
    ///     }
    /// }
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::builder().add_package::<MyPackage>().start().await.unwrap();
    /// }
    /// ```
    pub fn add_package<T>(mut self) -> Self
    where
        T: ActPackage + Clone + 'static,
    {
        let package_register = ActPackageRegister::new::<T>();
        self.packages.push(package_register);
        self
    }

    /// Pre-register a snapshot-backed sealed-data target before `start()`.
    ///
    /// Data is fed later through [`Engine::snapshot`](crate::Engine::snapshot)
    /// (message-channel adapters or the embedding application) and is sealed
    /// into tasks at their prepare from the local cache — no network I/O on
    /// the scheduling path.
    pub fn add_snapshot(mut self, name: &str, options: SnapshotOptions) -> Self {
        self.snapshots.push((name.to_string(), options));
        self
    }

    /// set the store
    ///
    /// The store backend is created externally and set here. When unset, an
    /// in-memory store is used. Only one store can be set — calling this
    /// again panics.
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{Engine, MemoryStore};
    /// use std::sync::Arc;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::builder()
    ///         .set_store(Arc::new(MemoryStore::new()))
    ///         .start()
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    ///
    /// The persistent backends live in the `acts-store` crate — enable its
    /// matching feature and import the backend from there, e.g. with feature
    /// `sqlite`: `use acts_store::SqliteStore;
    /// set_store(Arc::new(SqliteStore::open(path).await?))`. Any type
    /// implementing [`KvStore`](crate::KvStore) is accepted.
    pub fn set_store(mut self, store: Arc<dyn KvStore>) -> Self {
        assert!(
            self.store.is_none(),
            "store already set: only one backend is allowed"
        );
        self.store = Some(store);
        self
    }

    pub async fn start(self) -> crate::Result<Engine> {
        let Self {
            config,
            plugins,
            packages,
            snapshots,
            store,
            acl,
        } = self;
        let config = Arc::new(config);

        let runtime = Runtime::new(&config, store)?;
        let acl: Arc<dyn AccessControl> = acl.unwrap_or_else(|| Arc::new(crate::AnonymousAcl));
        let engine = Engine::with_runtime(config, runtime.clone(), acl)?;

        match engine.initialize(snapshots, plugins, packages).await {
            Ok(()) => {
                info!("engine started");
                Ok(engine)
            }
            Err(err) => {
                runtime.close().await;
                Err(err)
            }
        }
    }

    fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnonymousAcl, MemoryStore};

    #[test]
    fn builder_default_is_new() {
        let builder = EngineBuilder::default();
        assert_eq!(builder.config().log().dir, "log");
        assert!(builder.store.is_none());
        assert!(builder.acl.is_none());
    }

    #[test]
    fn builder_set_and_disable_acl() {
        let builder = EngineBuilder::new().set_acl(Arc::new(AnonymousAcl));
        assert!(builder.acl.is_some());
        assert!(builder.acl.as_ref().unwrap().enabled());

        let builder = EngineBuilder::new().disable_acl();
        assert!(!builder.acl.as_ref().unwrap().enabled());
    }

    #[test]
    fn builder_sets_workdir_and_worker_limits() {
        let builder = EngineBuilder::new()
            .workdir("/tmp/acts")
            .max_message_retry_times(3)
            .max_node_run_times(5)
            .scheduler_workers(2)
            .scheduler_queue_cap(16)
            .store_writer_workers(3)
            .store_writer_queue_cap(32)
            .tick_interval_secs(7)
            .cache_size(64)
            .log("logs", "DEBUG");

        let config = builder.config();
        assert_eq!(
            config.workdir().unwrap(),
            std::path::PathBuf::from("/tmp/acts")
        );
        assert_eq!(config.data.scheduler_workers, Some(2));
        assert_eq!(config.data.scheduler_queue_cap, Some(16));
        assert_eq!(config.data.store_writer_workers, Some(3));
        assert_eq!(config.data.store_writer_queue_cap, Some(32));
        assert_eq!(config.data.tick_interval_secs, Some(7));
        assert_eq!(config.data.cache_cap, Some(64));
        assert_eq!(config.max_message_retry_times(), 3);
        assert_eq!(config.max_node_run_times(), 5);
        assert_eq!(config.log().dir, "logs");
        assert_eq!(config.log().level, "DEBUG");
    }

    #[test]
    #[should_panic(expected = "store already set")]
    fn builder_refuses_a_second_store() {
        let _ = EngineBuilder::new()
            .set_store(Arc::new(MemoryStore::new()))
            .set_store(Arc::new(MemoryStore::new()));
    }

    #[test]
    fn builder_rejects_a_missing_config_source() {
        let path = std::env::temp_dir().join("acts-builder-missing-config-file.toml");
        let _ = std::fs::remove_file(&path);
        assert!(EngineBuilder::new().set_config_source(&path).is_err());
        assert!(
            EngineBuilder::new()
                .set_config_source(std::path::Path::new("."))
                .is_err()
        );
    }
}
