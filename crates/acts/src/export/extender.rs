use crate::{
    ActError, ActPackageMeta, ActRunAs, Result, env::ActUserVar, scheduler::Runtime, store::KvStore,
};
use core::fmt;
use std::sync::Arc;

#[derive(Clone)]
pub struct Extender {
    runtime: Arc<Runtime>,
}

impl fmt::Debug for Extender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extender").finish()
    }
}

impl Extender {
    pub(crate) fn new(runtime: &Arc<Runtime>) -> Self {
        Self {
            runtime: runtime.clone(),
        }
    }

    /// register module
    ///
    /// ## Example
    /// ```no_run
    /// use acts::Engine;
    /// mod test_module {
    ///   use acts::{ActUserVar, Vars, Result};
    ///   #[derive(Clone)]
    ///   pub struct TestModule;
    ///   impl ActUserVar for TestModule {
    ///     fn name(&self) -> String {
    ///         "my_var".to_string()
    ///     }
    ///
    ///     fn default_data(&self) -> Option<Vars> {
    ///         None
    ///     }
    ///   }
    /// }
    /// let engine = Engine::new().start().unwrap();
    /// let module = test_module::TestModule;
    /// engine.extender().register_var(&module);
    /// ```
    pub fn register_var<T: ActUserVar + Clone + 'static>(&self, module: &T) {
        self.runtime.env().register_var(module)
    }

    /// register package with meta definition
    /// ## Example
    /// ```no_run
    /// use acts::{ActPackage, ActPackageMeta, Vars};
    /// use serde::{Deserialize, Serialize};
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Clone, Deserialize, Serialize)]
    /// pub struct MyPackage {
    ///    a: i32,
    ///    b: Vec<String>,
    /// }
    /// impl ActPackage for MyPackage {
    ///     fn meta() -> ActPackageMeta {
    ///        ActPackageMeta {
    ///             id: "my_package",
    ///             name: "my package",
    ///             desc: "",
    ///             icon: "",
    ///             doc: "",
    ///             version: "0.1.0",
    ///             in_schema: json!({
    ///                 "type": "object",
    ///                 "properties": {
    ///                     "a": { "type": "number" },
    ///                     "b": { "type": "array" }
    ///                 }
    ///             }),
    ///             // refers to https://github.com/rjsf-team/react-jsonschema-form
    ///             ui_schema: None,
    ///             run_as: acts::ActRunAs::Irq,
    ///             resources: vec![],
    ///             catalog: acts::ActPackageCatalog::App,
    ///        }
    ///    }
    ///  }
    ///
    ///  #[tokio::main]
    ///  async fn main() {
    ///     let engine = acts::Engine::new().start().unwrap();
    ///     engine.extender().register_package(&MyPackage::meta()).unwrap();
    /// }
    pub fn register_package(&self, meta: &ActPackageMeta) -> Result<()> {
        if meta.run_as == ActRunAs::Func {
            return Err(ActError::Package(
                "package run_as must be one of Irq and Msg".to_string(),
            ));
        }
        let package = meta.into_data()?;
        self.runtime.cache().store().publish(&package)?;

        Ok(())
    }

    /// register collection
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{Engine, Result, KvStore, ScanOptions, data};
    /// use std::sync::Arc;
    ///
    /// pub struct MyStore;
    /// impl KvStore for MyStore {
    ///     fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
    ///         Ok(None)
    ///     }
    ///
    ///     fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
    ///         Ok(())
    ///     }
    ///
    ///     fn delete(&self, key: &str) -> Result<()> {
    ///         Ok(())
    ///     }
    ///
    ///     fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
    ///         Ok(vec![])
    ///     }
    ///
    /// }
    ///
    ///  #[tokio::main]
    ///  async fn main() {
    ///     let engine = acts::Engine::new().start().unwrap();
    ///     let store: Arc<dyn KvStore + 'static> = Arc::new(MyStore);
    ///     engine.extender().register_store(store);
    /// }
    pub fn register_store(&self, store: Arc<dyn KvStore + 'static>) {
        self.runtime.store().register(store);
    }
}
