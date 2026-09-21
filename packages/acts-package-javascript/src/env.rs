use crate::value::ActJsValue;
use acts::{ActError, Result};
use core::fmt;
use rquickjs::{Context as JsContext, Ctx as JsCtx, Runtime as JsRuntime};
use serde::de::DeserializeOwned;
use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

pub trait ActModule: Send + Sync {
    fn init(&self, ctx: &JsCtx<'_>) -> Result<()>;

    /// Install context-sensitive globals after [`ActModule::init`], before each
    /// expression runs. Built-in modules keep their static, realm-wide setup in
    /// `init` and put the task-dependent globals here.
    fn refresh(&self, _ctx: &JsCtx<'_>) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct Environment {
    pub(crate) modules: Arc<RwLock<Vec<Box<dyn ActModule>>>>,
}

impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment").finish()
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
        };
        env.init();
        env
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

        let runtime = JsRuntime::new().map_err(script_error)?;
        runtime.set_memory_limit(MAX_MEMORY);
        let ctx = JsContext::full(&runtime).map_err(script_error)?;

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
            global.remove("eval").map_err(script_error)?;

            let modules = self.modules.read().map_err(|_| {
                ActError::Script("environment modules lock is poisoned".to_string())
            })?;
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
                match rquickjs::CaughtError::from_error(&ctx, rquickjs::Error::Exception) {
                    rquickjs::CaughtError::Exception(exception) => {
                        return Err(ActError::Exception {
                            ecode: "".to_string(),
                            message: exception.message().unwrap_or_default(),
                        });
                    }
                    other => {
                        return Err(ActError::Script(format!(
                            "failed to read the thrown exception: {other}"
                        )));
                    }
                }
            }

            let value = result.map_err(script_error)?;
            let ret = serde_json::from_value::<T>(value.into())
                .map_err(|err| ActError::Convert(err.to_string()))?;
            Ok(ret)
        })
    }
}

pub(crate) fn script_error(err: rquickjs::Error) -> ActError {
    ActError::Script(err.to_string())
}
