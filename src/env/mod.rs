mod moudle;
#[cfg(test)]
mod tests;
mod value;

use crate::{ActError, Result, ShareLock, Vars};
use core::fmt;
use rquickjs::{Context as JsContext, Ctx as JsCtx, FromJs, Runtime as JsRuntime};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use self::value::ActValue;

/// ActModule to extend the js features
///
/// # Example
/// ```rust
///   use acts::{ActModule, Result};
///   #[derive(Clone)]
///   pub struct TestModule;
///   impl ActModule for TestModule {
///     fn init<'a>(&self, _ctx: &rquickjs::Ctx<'a>) -> Result<()> {
///         Ok(())
///     }
///   }
/// ```
pub trait ActModule: Send + Sync {
    fn init(&self, ctx: &JsCtx<'_>) -> Result<()>;
}

pub struct Enviroment {
    vars: ShareLock<Vars>,
    modules: ShareLock<Vec<Box<dyn ActModule>>>,
}

impl fmt::Debug for Enviroment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enviroment")
            .field("vars", &self.vars.read().unwrap())
            .finish()
    }
}

unsafe impl Send for Enviroment {}
unsafe impl Sync for Enviroment {}

impl Default for Enviroment {
    fn default() -> Self {
        Self::new()
    }
}

impl Enviroment {
    pub fn new() -> Self {
        let mut env = Enviroment {
            modules: Arc::new(RwLock::new(Vec::new())),
            vars: Arc::new(RwLock::new(Vars::new())),
        };
        env.init();
        env
    }

    #[cfg(test)]
    pub fn modules_count(&self) -> usize {
        self.modules.read().unwrap().len()
    }

    pub fn register_module<T: ActModule + Clone + 'static>(&self, module: &T) {
        let mut modules = self.modules.write().unwrap();
        modules.push(Box::new(module.clone()));
    }

    pub fn get<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        self.vars.read().unwrap().get::<T>(name)
    }

    pub fn set<T>(&self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        self.vars.write().unwrap().set(name, value);
    }

    pub fn update<F: FnOnce(&mut Vars)>(&self, f: F) {
        let mut vars = self.vars.write().unwrap();
        f(&mut vars);
    }

    pub fn eval<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let runtime = JsRuntime::new().unwrap();
        let ctx = JsContext::full(&runtime).unwrap();
        ctx.with(|ctx| {
            let modules = self.modules.read().unwrap();
            for m in modules.iter() {
                m.init(&ctx)?;
            }

            let result = ctx.eval::<ActValue, &str>(expr);
            if let Err(rquickjs::Error::Exception) = result {
                let exception = rquickjs::Exception::from_js(&ctx, ctx.catch()).unwrap();
                eprintln!("error: {exception:?}");
                return Err(ActError::Exception {
                    ecode: "".to_string(),
                    message: exception.message().unwrap_or_default(),
                });
            }

            let value = result.map_err(ActError::from)?;
            let ret = serde_json::from_value::<T>(value.into()).map_err(ActError::from)?;
            Ok(ret)
        })
    }
}
