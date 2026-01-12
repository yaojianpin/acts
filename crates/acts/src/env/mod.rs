mod moudle;
#[cfg(test)]
mod tests;
mod value;

use crate::{ActError, Result, ShareLock, Vars};
use core::fmt;
use rquickjs::{Context as JsContext, Ctx as JsCtx, FromJs, Runtime as JsRuntime};
use serde::de::DeserializeOwned;
use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use self::value::ActJsValue;

pub trait ActModule: Send + Sync {
    fn init(&self, ctx: &JsCtx<'_>) -> Result<()>;
}

/// User var trait
/// It can create user releated context data
///
/// # Example
/// ```rust
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
/// ```
pub trait ActUserVar: Send + Sync {
    /// global easier access name in js expression
    /// such as secrets.TOKEN, the secrets will be the name
    /// it will get the data by the name from task context
    fn name(&self) -> String;

    /// initialzie default data
    /// the data will be overridden by context vars
    fn default_data(&self) -> Option<Vars> {
        None
    }
}

#[derive(Clone)]
pub struct Enviroment {
    modules: ShareLock<Vec<Box<dyn ActModule>>>,
    pub(crate) user_vars: ShareLock<Vec<Box<dyn ActUserVar>>>,
}

impl fmt::Debug for Enviroment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enviroment").finish()
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
            user_vars: Arc::new(RwLock::new(Vec::new())),
        };
        env.init();
        env
    }

    #[cfg(test)]
    pub fn user_env_count(&self) -> usize {
        self.user_vars.read().unwrap().len()
    }

    pub fn register_var<T: ActUserVar + Clone + 'static>(&self, module: &T) {
        let mut user_envs = self.user_vars.write().unwrap();
        user_envs.push(Box::new(module.clone()));
    }

    pub fn eval<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let runtime = JsRuntime::new()?;
        runtime.set_memory_limit(10 * 1024 * 1024); // 10M
        let ctx = JsContext::full(&runtime)?;

        let start = Instant::now();
        ctx.with(|ctx| {
            let global = ctx.globals();
            // remove eval for safe reason
            global.remove("eval")?;

            let modules = self
                .modules
                .read()
                .map_err(|err| ActError::Script(err.to_string()))?;
            for m in modules.iter() {
                m.init(&ctx)?;
            }

            let result = ctx.eval::<ActJsValue, &str>(expr);
            if let Err(rquickjs::Error::Exception) = result {
                let exception = rquickjs::Exception::from_js(&ctx, ctx.catch()).unwrap();
                eprintln!("error: {exception:?}");
                return Err(ActError::Exception {
                    ecode: "".to_string(),
                    message: exception.message().unwrap_or_default(),
                });
            }

            // catches timeout
            if start.elapsed() > Duration::from_millis(15_000) {
                return Err(ActError::Script("Execution timeout".into()));
            }

            let value = result.map_err(ActError::from)?;
            let ret = serde_json::from_value::<T>(value.into()).map_err(ActError::from)?;
            Ok(ret)
        })
    }
}
