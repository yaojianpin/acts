use crate::{ActPackageDefinition, Principal, Result, env::ActUserVar, scheduler::Runtime};
use std::sync::Arc;

/// `ext:register_var` — install a user var module into the engine's
/// expression environment. Unlike the model/package names it has no wire
/// equivalent: only an embedder registers one.
pub(crate) const REGISTER_VAR: &str = "ext:register_var";

/// `pack:publish` — a package definition is written to the store, which is
/// what `pack().publish()` does too, so the two share one grant.
pub(crate) const REGISTER_PACKAGE: &str = "pack:publish";

/// The engine's own extension surface: registering user var modules and
/// publishing package definitions.
///
/// Both operations change what every run of the engine can express, so they
/// are checked like any other operation — against the principal this executor
/// was built with. An embedder that extends the engine it hosts passes its
/// own identity ([`Principal::unrestricted`]); a transport caller never
/// reaches here at all, because no action of the wire table maps to it.
#[derive(Clone)]
pub struct ExtExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl ExtExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
        }
    }

    /// register a user var module, so `$env`/expressions can read it
    ///
    /// ## Example
    /// ```no_run
    /// use acts::{Engine, Principal};
    /// mod test_module {
    ///   use acts::{ActUserVar, Vars};
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
    ///     let engine = Engine::builder().start().await.unwrap();
    ///     let module = test_module::TestModule;
    ///     // The engine this embedder owns acts as itself: `Engine::anonymous`
    ///     // would be the tokenless caller, which may not change the engine.
    ///     engine
    ///         .executor(&Principal::unrestricted())
    ///         .ext()
    ///         .register_var(&module)
    ///         .unwrap();
    /// }
    /// ```
    pub fn register_var<T: ActUserVar + Clone + 'static>(&self, module: &T) -> Result<()> {
        self.principal.check(REGISTER_VAR)?;
        self.runtime.env().register_var(module);
        Ok(())
    }

    /// publish a package definition into the package catalogue
    /// ## Example
    /// ```no_run
    /// use acts::{ActPackage, ActPackageDefinition, Engine, Principal, Vars};
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
    ///     let engine = acts::Engine::builder().start().await.unwrap();
    ///     engine
    ///         .executor(&Principal::unrestricted())
    ///         .ext()
    ///         .register_package(&MyPackage::definition())
    ///         .await
    ///         .unwrap();
    /// }
    /// ```
    pub async fn register_package(&self, def: &ActPackageDefinition) -> Result<()> {
        self.principal.check(REGISTER_PACKAGE)?;
        let package = def.into_data()?;
        let ret = self.runtime.cache().store().publish(&package).await?;
        if ret {
            self.runtime.schema_cache().invalidate_package(&package.id);
        }

        Ok(())
    }
}
