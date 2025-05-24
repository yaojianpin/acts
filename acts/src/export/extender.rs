use crate::{
    ActError, ActModule, ActPackageMeta, ActRunAs, DbCollection, Result, scheduler::Runtime,
    store::DbCollectionIden,
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
    ///   use acts::{ActModule, Result};
    ///   #[derive(Clone)]
    ///   pub struct TestModule;
    ///   impl ActModule for TestModule {
    ///     fn init<'a>(&self, _ctx: &rquickjs::Ctx<'a>) -> Result<()> {
    ///         Ok(())
    ///     }
    ///   }
    /// }
    /// let engine = Engine::new().start();
    /// let module = test_module::TestModule;
    /// engine.extender().register_module(&module);
    /// ```
    pub fn register_module<T: ActModule + Clone + 'static>(&self, module: &T) {
        self.runtime.env().register_module(module)
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
    ///             name: "my_package",
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
    ///             run_as: acts::ActRunAs::Irq,
    ///             resources: vec![],
    ///             catalog: acts::ActPackageCatalog::App,
    ///        }
    ///    }
    ///  }
    ///
    ///  #[tokio::main]
    ///  async fn main() {
    ///     let engine = acts::Engine::new().start();
    ///     engine.extender().register_package(&MyPackage::meta());
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
    /// use acts::{Engine, Result, DbCollection, data};
    /// use std::sync::Arc;
    ///
    /// pub struct MyCollection;
    /// impl DbCollection for MyCollection {
    ///    type Item = data::Event;
    ///     fn exists(&self, id: &str) -> Result<bool> {
    ///         Ok(true)
    ///     }
    ///     
    ///     fn find(&self, id: &str) -> Result<Self::Item> {
    ///         Ok(data::Event {
    ///             id: todo!(),
    ///             name: todo!(),
    ///             mid: todo!(),
    ///             ver: todo!(),
    ///             uses: todo!(),
    ///             params: todo!(),
    ///             create_time: todo!(),
    ///             timestamp: todo!(),
    ///         })
    ///     }
    ///     
    ///     fn query(&self, q: &acts::query::Query) -> Result<acts::PageData<Self::Item>> {
    ///         Ok(acts::PageData {
    ///             count: 0,
    ///             page_num: 0,
    ///             page_count: 0,
    ///             page_size: 0,
    ///             rows: vec![],
    ///         })
    ///     }
    ///     
    ///     fn create(&self, data: &Self::Item) -> Result<bool> {
    ///         Ok(true)
    ///     }
    ///     
    ///     fn update(&self, data: &Self::Item) -> Result<bool> {
    ///         Ok(true)
    ///     }
    ///     
    ///     fn delete(&self, id: &str) -> Result<bool> {
    ///         Ok(true)
    ///     }
    /// }
    ///
    ///  #[tokio::main]
    ///  async fn main() {
    ///     let engine = acts::Engine::new().start();
    ///     let collection: Arc<dyn DbCollection<Item = data::Event> + Send + Sync> = Arc::new(MyCollection);
    ///     engine.extender().register_collection(collection);
    /// }
    pub fn register_collection<DATA>(
        &self,
        collection: Arc<dyn DbCollection<Item = DATA> + Send + Sync + 'static>,
    ) where
        DATA: DbCollectionIden + 'static,
    {
        self.runtime.store().register(collection);
    }
}
