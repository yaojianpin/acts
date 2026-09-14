mod module;
#[cfg(test)]
mod tests;
mod value;

use crate::{ActError, Result, ShareLock, Vars};
use core::fmt;
use parking_lot::RwLock;
use rquickjs::{Context as JsContext, Ctx as JsCtx, FromJs, Runtime as JsRuntime};
use serde::de::DeserializeOwned;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use self::value::ActJsValue;

pub trait ActModule: Send + Sync {
    fn init(&self, ctx: &JsCtx<'_>) -> Result<()>;

    /// Install context-sensitive globals after [`ActModule::init`], before each
    /// expression runs. Built-in modules keep their static, realm-wide setup in
    /// `init` and put the task-dependent globals here.
    fn refresh(&self, _ctx: &JsCtx<'_>) -> Result<()> {
        Ok(())
    }
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
pub struct Environment {
    modules: ShareLock<Vec<Box<dyn ActModule>>>,
    pub(crate) user_vars: ShareLock<Vec<Box<dyn ActUserVar>>>,
}

impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enviroment").finish()
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        let mut env = Environment {
            modules: Arc::new(RwLock::new(Vec::new())),
            user_vars: Arc::new(RwLock::new(Vec::new())),
        };
        env.init();
        env
    }

    #[cfg(test)]
    pub fn user_env_count(&self) -> usize {
        self.user_vars.read().len()
    }

    pub fn register_var<T: ActUserVar + Clone + 'static>(&self, module: &T) {
        let mut user_envs = self.user_vars.write();
        user_envs.push(Box::new(module.clone()));
    }

    pub fn register_module(&self, module: Box<dyn ActModule>) {
        self.modules.write().push(module);
    }

    /// Evaluate `expr` in its own QuickJS runtime and context (realm).
    ///
    /// Nothing is pooled: a reused runtime carries state across contexts
    /// (rquickjs caches class and callable prototypes on the runtime, and a
    /// realm's intrinsics stay reachable through them), and a reused context
    /// cannot be scrubbed reliably — an expression may leave state behind that
    /// a global-property snapshot does not cover (built-in prototypes and
    /// namespace objects, module-provided proxies and objects,
    /// non-configurable globals). The next expression, possibly another task or
    /// tenant, would read it. A fresh runtime shares no mutable state, so
    /// isolation no longer depends on enumerating what an expression touched.
    pub fn eval<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        const TIMEOUT: Duration = Duration::from_millis(15_000);
        const MAX_MEMORY: usize = 10 * 1024 * 1024;

        let runtime = JsRuntime::new()?;
        runtime.set_memory_limit(MAX_MEMORY);
        let ctx = JsContext::full(&runtime)?;

        self.eval_in_context(&ctx, expr, TIMEOUT)
    }

    fn eval_in_context<T>(&self, ctx: &JsContext, expr: &str, timeout: Duration) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let start = Instant::now();
        // QuickJS invokes the handler regularly (every 10_000 instructions)
        // while running script; returning true aborts execution with an
        // uncatchable exception, so an infinite loop cannot hang the process
        // past the deadline.
        ctx.runtime()
            .set_interrupt_handler(Some(Box::new(move || start.elapsed() > timeout)));

        ctx.with(|ctx| {
            let global = ctx.globals();
            // remove eval for safe reason
            global.remove("eval")?;

            let modules = self.modules.read();
            for m in modules.iter() {
                m.init(&ctx)?;
            }
            for m in modules.iter() {
                m.refresh(&ctx)?;
            }

            // Evaluate inside a block so a top-level declaration stays local to
            // the expression; the block's completion value is the result, just
            // like a top-level program.
            let script = format!("{{\n{expr}\n}}");
            let result = ctx.eval::<ActJsValue, _>(script);
            if start.elapsed() > timeout {
                return Err(ActError::Script("Execution timeout".into()));
            }
            if let Err(rquickjs::Error::Exception) = result {
                match rquickjs::Exception::from_js(&ctx, ctx.catch()) {
                    Ok(exception) => {
                        return Err(ActError::Exception {
                            ecode: "".to_string(),
                            message: exception.message().unwrap_or_default(),
                        });
                    }
                    Err(exception) => {
                        return Err(ActError::Script(format!(
                            "failed to read the thrown exception: {exception}"
                        )));
                    }
                }
            }

            let value = result.map_err(ActError::from)?;
            let ret = serde_json::from_value::<T>(value.into()).map_err(ActError::from)?;
            Ok(ret)
        })
    }
}
