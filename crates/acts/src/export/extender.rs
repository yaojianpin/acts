use crate::{ActPackageDefinition, Result, env::ActUserVar, scheduler::Runtime};
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
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new().start().await.unwrap();
    ///     let module = test_module::TestModule;
    ///     engine.extender().register_var(&module);
    /// }
    /// ```
    pub fn register_var<T: ActUserVar + Clone + 'static>(&self, module: &T) {
        self.runtime.env().register_var(module)
    }

    /// register package with meta definition
    /// ## Example
    /// ```no_run
    /// use acts::{ActPackage, ActPackageDefinition, Vars};
    /// use serde::{Deserialize, Serialize};
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Clone, Deserialize, Serialize)]
    /// pub struct MyPackage {
    ///    a: i32,
    ///    b: Vec<String>,
    /// }
    /// impl ActPackage for MyPackage {
    ///     fn definition() -> ActPackageDefinition {
    ///        ActPackageDefinition {
    ///             id: "my_package",
    ///             name: "my package",
    ///             desc: "",
    ///             icon: "",
    ///             doc: "",
    ///             version: "0.1.0",
    ///             schema: json!({
    ///                 "type": "object",
    ///                 "properties": {
    ///                     "a": { "type": "number" },
    ///                     "b": { "type": "array" }
    ///                 }
    ///             }),
    ///             // refers to https://github.com/rjsf-team/react-jsonschema-form
    ///             options: None,
    ///             run_as: acts::ActRunAs::Irq,
    ///             resources: vec![],
    ///             catalog: acts::ActPackageCatalog::App,
    ///        }
    ///    }
    ///
    ///    fn new(_config: &acts::Config) -> acts::Result<Self> {
    ///        Ok(Self { a: 0, b: vec![] })
    ///    }
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = acts::Engine::new().start().await.unwrap();
    ///     engine.extender()
    ///         .register_package(&MyPackage::definition())
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    pub async fn register_package(&self, def: &ActPackageDefinition) -> Result<()> {
        let package = def.into_data()?;
        self.runtime.cache().store().publish(&package).await?;

        Ok(())
    }
}
