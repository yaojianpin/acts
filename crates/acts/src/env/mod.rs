mod functions;
#[cfg(test)]
mod tests;

use crate::{ActError, Context, Result, ShareLock, Vars};
use acts_expr::{Context as ExprContext, Expr};
use core::fmt;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use std::sync::Arc;

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
    /// global easier access name in the expression
    /// such as `secrets.TOKEN`, the `secrets` will be the name;
    /// the data is read from the task context by that name.
    fn name(&self) -> String;

    /// initialzie default data
    /// the data will be overridden by context vars
    fn default_data(&self) -> Option<Vars> {
        None
    }
}

/// A method an expression can call, registered by a plugin or embedder.
///
/// The name is what the expression writes: `owner_of(...)` or, by the
/// engine's convention for built-ins, `$owner_of(...)`. A method that needs
/// the acting task reads it through
/// [`Context::try_with_current`](crate::Context::try_with_current) (the
/// evaluation runs on the task's scope); the engine passes no separate
/// context, so a method is also usable from a plain [`Environment::eval`].
///
/// ```rust
/// use acts::{ActFunction, Result};
/// use serde_json::{Value as JsonValue, json};
///
/// #[derive(Clone)]
/// struct Doubled;
///
/// impl ActFunction for Doubled {
///     fn name(&self) -> String {
///         "doubled".to_string()
///     }
///
///     fn call(&self, args: &[JsonValue]) -> Result<JsonValue> {
///         let value = args.first().and_then(JsonValue::as_i64).unwrap_or(0);
///         Ok(json!(value * 2))
///     }
/// }
/// ```
pub trait ActFunction: Send + Sync {
    /// The name the expression calls.
    fn name(&self) -> String;

    /// Call the method with the expression's argument values. The returned
    /// JSON value is converted back into the expression's value.
    fn call(&self, args: &[JsonValue]) -> Result<JsonValue>;
}

/// The built-in `secrets` user var: reads the task's `secrets` data.
#[derive(Clone)]
struct SecretsVar;

impl ActUserVar for SecretsVar {
    fn name(&self) -> String {
        "secrets".to_string()
    }
}

#[derive(Clone)]
pub struct Environment {
    pub(crate) user_vars: ShareLock<Vec<Box<dyn ActUserVar>>>,
    /// Plugin-registered expression methods, in registration order. A name is
    /// answered by the first registration, exactly like a user var.
    functions: ShareLock<Vec<Arc<dyn ActFunction>>>,
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
        Environment {
            user_vars: Arc::new(RwLock::new(vec![Box::new(SecretsVar)])),
            functions: Arc::new(RwLock::new(Vec::new())),
        }
    }

    #[cfg(test)]
    pub fn user_env_count(&self) -> usize {
        self.user_vars.read().len()
    }

    pub fn register_var<T: ActUserVar + Clone + 'static>(&self, module: &T) {
        let mut user_envs = self.user_vars.write();
        user_envs.push(Box::new(module.clone()));
    }

    /// Register an expression method. The first registration of a name wins;
    /// a later duplicate is kept but never resolved.
    pub fn register_function<T: ActFunction + Clone + 'static>(&self, function: &T) {
        self.functions.write().push(Arc::new(function.clone()));
    }

    /// The first registered method under `name`.
    pub(crate) fn function(&self, name: &str) -> Option<Arc<dyn ActFunction>> {
        self.functions
            .read()
            .iter()
            .find(|function| function.name() == name)
            .cloned()
    }

    /// One user var's data — `default_data` overlaid with what the current
    /// task holds under that name — or `None` if no var is registered by it.
    pub fn user_var(&self, name: &str) -> Option<Vars> {
        let mut data = {
            let vars = self.user_vars.read();
            let var = vars.iter().find(|var| var.name() == name)?;

            var.default_data().unwrap_or_default()
        };

        if let Ok(Some(vars)) =
            Context::try_with_current(|ctx| ctx.task().find::<Vars>(&ctx.proc, name))
        {
            for (k, v) in vars.iter() {
                data.set(k, v);
            }
        }

        Some(data)
    }

    /// Evaluate an expression, returning the value deserialized into `T`.
    ///
    /// The engine's `$`-prefixed built-ins (`$env`, `$get`, `$profile`, …) are
    /// ordinary identifiers to the evaluator, so an expression reads exactly
    /// as it is written. Only the names the expression references are resolved
    /// (see [`functions::inject`]), and each evaluation gets its own context,
    /// so nothing an expression injects is visible to the next one.
    pub fn eval<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let program = Expr::compile(expr).map_err(|err| ActError::Script(err.to_string()))?;

        let mut ctx = ExprContext::new();
        functions::inject(self, &mut ctx, &program.references())?;

        let value = program
            .eval(&ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;
        let json = value
            .to_json()
            .map_err(|err| ActError::Script(err.to_string()))?;

        serde_json::from_value::<T>(json).map_err(ActError::from)
    }
}
