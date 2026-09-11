mod moudle;
#[cfg(test)]
mod tests;
mod value;

use crate::{ActError, Result, ShareLock, Vars};
use core::fmt;
use parking_lot::{Mutex, RwLock};
use rquickjs::{CatchResultExt, Context as JsContext, Ctx as JsCtx, FromJs, Runtime as JsRuntime};
use serde::de::DeserializeOwned;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use self::value::ActJsValue;

pub trait ActModule: Send + Sync {
    fn init(&self, ctx: &JsCtx<'_>) -> Result<()>;

    /// Install context-sensitive globals before every expression. Built-in
    /// modules keep static setup in [`ActModule::init`] so repeated expressions
    /// do not redeclare native modules.
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
pub struct Enviroment {
    modules: ShareLock<Vec<Box<dyn ActModule>>>,
    pub(crate) user_vars: ShareLock<Vec<Box<dyn ActUserVar>>>,
    /// Third-party modules may install arbitrary state in `init`; preserve the
    /// old one-context-per-expression behavior for them.
    has_custom_modules: Arc<AtomicBool>,
    runtimes: Arc<Mutex<Vec<JsRuntime>>>,
    contexts: Arc<Mutex<Vec<(JsContext, bool)>>>,
}

impl fmt::Debug for Enviroment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enviroment").finish()
    }
}

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
            has_custom_modules: Arc::new(AtomicBool::new(false)),
            runtimes: Arc::new(Mutex::new(Vec::new())),
            contexts: Arc::new(Mutex::new(Vec::new())),
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
        let mut modules = self.modules.write();
        modules.push(module);
        self.has_custom_modules.store(true, Ordering::Release);
        // Existing pooled contexts only know the built-ins' static state. Drop
        // them so subsequent evaluations use the conservative fresh-context
        // path while the custom module is registered.
        self.contexts.lock().clear();
    }

    pub fn eval<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        if self.has_custom_modules.load(Ordering::Acquire) {
            self.eval_with_new_context(expr)
        } else {
            self.eval_with_pooled_context(expr)
        }
    }
}

const GLOBAL_SNAPSHOT_INIT: &str = r#"
(function() {
    const names = new Set();
    const descriptors = {};
    for (const name of Object.getOwnPropertyNames(globalThis)) {
        names.add(name);
        const descriptor = Object.getOwnPropertyDescriptor(globalThis, name);
        Object.freeze(descriptor);
        descriptors[name] = descriptor;
    }
    Object.freeze(names);
    Object.freeze(descriptors);
    Object.defineProperty(globalThis, "__acts_global_state", {
        value: { names, descriptors },
        writable: false,
        enumerable: false,
        configurable: false,
    });
})()
"#;

const GLOBAL_SNAPSHOT_RESET: &str = r#"
(function() {
    const snapshot = globalThis.__acts_global_state;
    if (!snapshot) return;
    for (const name of Object.getOwnPropertyNames(globalThis)) {
        if (name !== "__acts_global_state" && !snapshot.names.has(name)) {
            try { delete globalThis[name]; } catch {}
            const descriptor = Object.getOwnPropertyDescriptor(globalThis, name);
            if (descriptor && descriptor.writable) globalThis[name] = undefined;
        }
    }
    for (const [name, descriptor] of Object.entries(snapshot.descriptors)) {
        try { Object.defineProperty(globalThis, name, descriptor); } catch {}
    }
})()
"#;

impl Enviroment {
    fn eval_with_new_context<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        const TIMEOUT: Duration = Duration::from_millis(15_000);
        const MAX_POOLED_RUNTIMES: usize = 32;

        let runtime = match self.runtimes.lock().pop() {
            Some(runtime) => runtime,
            None => JsRuntime::new()?,
        };
        runtime.set_memory_limit(10 * 1024 * 1024);
        let ctx = JsContext::full(&runtime)?;

        let result = self.eval_in_context(&ctx, expr, TIMEOUT, false);
        drop(ctx);
        runtime.run_gc();

        let mut runtimes = self.runtimes.lock();
        if result.is_ok() && runtimes.len() < MAX_POOLED_RUNTIMES {
            runtimes.push(runtime);
        }
        drop(runtimes);

        result
    }

    fn eval_with_pooled_context<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        const TIMEOUT: Duration = Duration::from_millis(15_000);
        const MAX_POOLED_CONTEXTS: usize = 32;

        let (ctx, initialized) = match self.contexts.lock().pop() {
            Some(pooled) => pooled,
            None => {
                let runtime = JsRuntime::new()?;
                (JsContext::full(&runtime)?, false)
            }
        };
        ctx.runtime().set_memory_limit(10 * 1024 * 1024);

        let start = Instant::now();
        let result = self.eval_in_context(&ctx, expr, TIMEOUT, initialized);
        {
            let mut contexts = self.contexts.lock();
            // A failed expression may have timed out or left arbitrary global
            // changes; discard the context rather than risking contamination.
            if result.is_ok() && start.elapsed() <= TIMEOUT && contexts.len() < MAX_POOLED_CONTEXTS
            {
                contexts.push((ctx, true));
            }
        }

        result
    }

    fn eval_in_context<T>(
        &self,
        ctx: &JsContext,
        expr: &str,
        timeout: Duration,
        initialized: bool,
    ) -> Result<T>
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

        let result = ctx.with(|ctx| {
            let global = ctx.globals();
            // remove eval for safe reason
            global.remove("eval")?;

            let modules = self.modules.read();
            if !initialized {
                for m in modules.iter() {
                    m.init(&ctx)?;
                }
                ctx.eval::<(), _>(GLOBAL_SNAPSHOT_INIT)?;
            } else {
                ctx.eval::<(), _>(GLOBAL_SNAPSHOT_RESET)
                    .catch(&ctx)
                    .map_err(|err| ActError::Script(err.to_string()))?;
            }
            for m in modules.iter() {
                m.refresh(&ctx)?;
            }

            // A block scope prevents global lexical declarations (`let`,
            // `const`, and `class`) from leaking between evaluations while a
            // context is pooled. Block completion values preserve expression
            // results just like a top-level program.
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
        });

        ctx.runtime().run_gc();
        result
    }
}
