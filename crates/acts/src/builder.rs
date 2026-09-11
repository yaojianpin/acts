use crate::snapshot::SnapshotOptions;
use crate::{
    ActPackage, ActPlugin, Config, Engine, config::ConfigLog, package::ActPackageRegister,
    scheduler::Runtime, store::KvStore,
};
use std::{path::Path, sync::Arc};
use tracing::{info, warn};

pub struct EngineBuilder {
    config: Config,
    plugins: Vec<Arc<dyn ActPlugin>>,
    packages: Vec<ActPackageRegister>,
    snapshots: Vec<(String, SnapshotOptions)>,
    store: Option<Arc<dyn KvStore>>,
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

    pub fn log(mut self, dir: &str, level: &str) -> Self {
        self.config_mut().data.log = Some(ConfigLog {
            dir: dir.to_string(),
            level: level.to_string(),
        });
        self
    }

    pub fn cache_size(mut self, size: i64) -> Self {
        self.config_mut().data.cache_cap = Some(size);
        self
    }

    pub fn tick_interval_secs(mut self, secs: i64) -> Self {
        self.config_mut().data.tick_interval_secs = Some(secs);
        self
    }

    pub fn max_message_retry_times(mut self, retry_times: i32) -> Self {
        self.config_mut().data.max_message_retry_times = Some(retry_times);
        self
    }
    /// bound the times a tree node can be executed in one process (protects
    /// against unbounded task creation caused by a node self-loop or a cyclic
    /// `next`); 0 disables the check
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
        } = self;
        let config = Arc::new(config);

        let runtime = Runtime::new(&config, store)?;
        let engine = Engine::with_runtime(config, runtime.clone());

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
