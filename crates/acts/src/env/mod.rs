mod functions;
#[cfg(test)]
mod tests;

use crate::{ActError, Context, Result, ShareLock, Vars};
use cel::Program;
use core::fmt;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
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

    /// Resolve every registered user var (the built-in `secrets` and any
    /// embedder-registered var) to its data: the var's `default_data` overlaid
    /// with the value the current task holds under the var's name.
    pub fn user_vars(&self) -> Vec<(String, Vars)> {
        self.user_vars
            .read()
            .iter()
            .map(|var| {
                let name = var.name();
                let mut data = var.default_data().unwrap_or_default();
                if let Ok(Some(vars)) =
                    Context::try_with_current(|ctx| ctx.task().find::<Vars>(&name))
                {
                    for (k, v) in vars.iter() {
                        data.set(k, v);
                    }
                }
                (name, data)
            })
            .collect()
    }

    /// Evaluate a CEL expression, returning the value deserialized into `T`.
    ///
    /// The engine's `$`-prefixed built-ins (`$env`, `$get`, `$profile`, …)
    /// are accepted as in the workflow DSL and rewritten to valid CEL
    /// identifiers before compilation; task vars, user vars and step ids are
    /// injected as bare identifiers.
    pub fn eval<T>(&self, expr: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let expr = functions::rewrite(expr);
        let program = Program::compile(&expr).map_err(|err| ActError::Script(err.to_string()))?;

        // The function registry is shared and initialized once; a child scope
        // carries only this evaluation's variables.
        let mut ctx = functions::root().new_inner_scope();
        functions::inject_vars(self, &mut ctx)?;

        let value = program
            .execute(&ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;
        let json = value
            .json()
            .map_err(|err| ActError::Script(err.to_string()))?;
        serde_json::from_value::<T>(json).map_err(ActError::from)
    }
}
