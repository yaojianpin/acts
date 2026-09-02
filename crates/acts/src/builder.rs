use crate::{
    ActPackage, ActPlugin, Config, Engine, config::ConfigLog, package::ActPackageRegister,
    store::KvStore,
};
use std::{path::Path, sync::Arc};

pub struct EngineBuilder {
    config: Config,
    plugins: Vec<Arc<dyn ActPlugin>>,
    packages: Vec<ActPackageRegister>,
    resolvers: Vec<(String, Arc<dyn crate::config::ConfigResolver>)>,
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
            config = Config::create(file);
        }

        Self {
            config,
            plugins: Vec::new(),
            packages: Vec::new(),
            resolvers: Vec::new(),
            store: None,
        }
    }

    pub fn set_config(mut self, config: &Config) -> Self {
        self.config = config.clone();
        self
    }

    pub fn set_config_source(mut self, source: &Path) -> Self {
        self.config = Config::create(source);
        self
    }

    pub fn log(mut self, dir: &str, level: &str) -> Self {
        self.config.data.log = Some(ConfigLog {
            dir: dir.to_string(),
            level: level.to_string(),
        });
        self
    }

    pub fn cache_size(mut self, size: i64) -> Self {
        self.config.data.cache_cap = Some(size);
        self
    }

    pub fn tick_interval_secs(mut self, secs: i64) -> Self {
        self.config.data.tick_interval_secs = Some(secs);
        self
    }

    pub fn max_message_retry_times(mut self, retry_times: i32) -> Self {
        self.config.data.max_message_retry_times = Some(retry_times);
        self
    }
    /// bound the times a tree node can be executed in one process (protects
    /// against unbounded task creation caused by a node self-loop or a cyclic
    /// `next`); 0 disables the check
    pub fn max_node_run_times(mut self, times: i64) -> Self {
        self.config.data.max_node_run_times = Some(times);
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
    ///         engine.channel().on_start(|e| {});
    ///         engine.channel().on_complete(|e| {});
    ///         engine.channel().on_message(|e| {});
    ///         Ok(())       
    ///     }
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::builder().add_plugin(&TestPlugin::new()).build().start().unwrap();
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
    ///     let engine = Engine::builder().add_package::<MyPackage>().build().start().unwrap();
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

    /// register config resolver
    ///
    /// Resolvers are invoked at `proc.start()` on each task to inject
    /// tenant-scoped configuration into sealed data, which inherits
    /// from parent tasks.
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ConfigResolver, Engine, Result, Vars};
    /// use std::sync::Arc;
    ///
    /// struct MyResolver {
    ///     data: Vars,
    /// }
    ///
    /// #[async_trait::async_trait]
    /// impl ConfigResolver for MyResolver {
    ///     async fn resolve(&self, _ctx: &Vars) -> Result<Vars> {
    ///         Ok(self.data.clone())
    ///     }
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let resolver = Arc::new(MyResolver {
    ///         data: Vars::new()
    ///             .with("secrets", Vars::new().with("TOKEN", "abc123")),
    ///     });
    ///     let engine = Engine::builder()
    ///         .add_resolver("profile", resolver)
    ///         .build()
    ///         .start()
    ///         .unwrap();
    /// }
    /// ```
    pub fn add_resolver(
        mut self,
        name: &str,
        resolver: Arc<dyn crate::config::ConfigResolver>,
    ) -> Self {
        self.resolvers.push((name.to_string(), resolver));
        self
    }

    /// set the store
    ///
    /// The `store-*` cargo features only control which backend structs are
    /// compiled and exported; create the backend externally and set it here.
    /// When unset, an in-memory store is used. Only one store can be set —
    /// calling this again panics.
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
    ///         .build()
    ///         .start()
    ///         .unwrap();
    /// }
    /// ```
    ///
    /// `SqliteStore`, `PostgresStore`, `RedisStore`, `NatsStore` and
    /// `SledStore` are exported from the crate when the matching `store-*`
    /// feature is enabled, e.g. `set_store(Arc::new(SqliteStore::open(path)?))`.
    pub fn set_store(mut self, store: Arc<dyn KvStore>) -> Self {
        assert!(
            self.store.is_none(),
            "store already set: only one backend is allowed"
        );
        self.store = Some(store);
        self
    }

    pub fn build(self) -> Engine {
        Engine::new()
            .with_config(&self.config)
            .set_plugins(self.plugins.clone())
            .set_packages(self.packages.clone())
            .set_resolvers(self.resolvers.clone())
            .set_store(self.store)
    }
}
