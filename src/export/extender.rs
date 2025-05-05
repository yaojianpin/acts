use core::fmt;
use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::{
    ActModule, ActPackage, Result, StoreAdapter,
    package::{ActPackageFn, ActPackageRegister},
    scheduler::Runtime,
};

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

    pub fn register_package<T>(&self) -> Result<()>
    where
        T: ActPackage + ActPackageFn + ActPackage + 'static + Clone,
        T: DeserializeOwned,
    {
        let meta = T::meta();
        let package = meta.into_data()?;
        self.runtime.cache().store().publish(&package)?;
        println!("register_package: {}", meta.name);

        self.runtime
            .package()
            .register(meta.name, &ActPackageRegister::new::<T>());

        Ok(())
    }

    pub fn register_store<T>(&self, store: T) -> Result<()>
    where
        T: StoreAdapter + 'static + Clone,
    {
        self.runtime.adapter().set_store(Arc::new(store));
        Ok(())
    }

    // pub fn plugins(&self) -> Arc<Mutex<Vec<Box<dyn ActPlugin>>>> {
    //     self.plugins.clone()
    // }
}
