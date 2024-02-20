mod moudle;
mod refenv;

#[cfg(test)]
mod tests;

use crate::{ActError, ActModule, ActValue, Result, Vars};
pub use refenv::RefEnv;
use rhai::Engine as ScriptEngine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use tracing::debug;

#[derive(Debug, Default)]
pub struct Enviroment {
    scr: Mutex<ScriptEngine>,
    scope: RefCell<rhai::Scope<'static>>,
    vars: RefCell<Vars>,
}

unsafe impl Send for Enviroment {}
unsafe impl Sync for Enviroment {}

impl Enviroment {
    pub fn new() -> Arc<Self> {
        let scope = rhai::Scope::new();
        let env = Enviroment {
            scr: Mutex::new(ScriptEngine::new()),
            scope: RefCell::new(scope),
            vars: RefCell::new(Vars::new()),
        };
        env.init();
        Arc::new(env)
    }

    pub fn create_ref(self: &Arc<Self>, id: &str) -> Arc<RefEnv> {
        Arc::new(RefEnv::new(self, id))
    }

    pub fn init(&self) {
        debug!("env::init");
        self.registry_collection_module();
        self.registry_console_module();
        self.registry_env_module();
    }

    pub fn vars(&self) -> Vars {
        self.vars.borrow().clone()
    }

    pub fn data(&self, id: &str) -> Option<Vars> {
        self.vars.borrow().get(id)
    }

    pub fn set_data(&self, id: &str, values: &Vars) {
        if !self.vars.borrow().contains_key(id) {
            self.vars.borrow_mut().set(id, Vars::new());
        }
        if let Some(vars) = self.vars.borrow_mut().get_mut(id) {
            if let Some(data) = vars.as_object_mut() {
                for (name, value) in values {
                    data.entry(name)
                        .and_modify(|v| *v = value.clone())
                        .or_insert(value.clone());
                }
            }
        }
    }

    pub fn update_data(&self, id: &str, name: &str, value: &ActValue) -> bool {
        if let Some(vars) = self.vars.borrow_mut().get_mut(id) {
            if let Some(data) = vars.as_object_mut() {
                if data.contains_key(name) {
                    data.entry(name).and_modify(|v| *v = value.clone());
                    return true;
                }
            }
        }

        false
    }

    pub fn root(&self) -> Vars {
        if let Some(vars) = self.data("$") {
            return vars;
        }

        Vars::new()
    }

    pub fn append(&self, vars: &Vars) {
        let mut env = self.vars.borrow_mut();
        for (name, v) in vars {
            env.entry(name.to_string())
                .and_modify(|i| *i = v.clone())
                .or_insert(v.clone());
        }
    }

    #[allow(unused)]
    pub fn run(&self, script: &str) -> Result<bool> {
        let scr = self.scr.lock().unwrap();
        let mut scope = self.scope.borrow_mut();
        match scr.run_with_scope(&mut scope, script) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn eval<T: rhai::Variant + Clone>(&self, expr: &str) -> Result<T> {
        let scr = self.scr.lock().unwrap();
        let mut scope = self.scope.borrow_mut();
        match scr.eval_with_scope::<T>(&mut scope, expr) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    fn get<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        if let Some(value) = self.get_value(name) {
            if let Ok(v) = serde_json::from_value::<T>(value) {
                return Some(v);
            }
        }

        None
    }

    pub fn get_value(&self, name: &str) -> Option<ActValue> {
        let vars = self.vars.borrow();
        match vars.get_value(name) {
            Some(v) => Some(v.clone()),
            None => None,
        }
    }

    #[allow(unused)]
    fn set<T>(&self, name: &str, value: T)
    where
        T: Serialize,
    {
        self.set_value(name, json!(value));
    }

    #[allow(unused)]
    fn set_value(&self, name: &str, value: ActValue) {
        let mut vars = self.vars.borrow_mut();
        vars.entry(name.to_string())
            .and_modify(|i| *i = value.clone())
            .or_insert(value);
    }

    #[allow(unused)]
    fn remove(&self, name: &str) {
        let mut vars = self.vars.borrow_mut();
        vars.remove(name);
    }

    fn register_module(&self, name: impl AsRef<str>, module: ActModule) {
        let scr = &mut *self.scr.lock().unwrap();
        scr.register_static_module(name, module.into());
    }

    fn register_global_module(&self, module: ActModule) {
        let scr = &mut *self.scr.lock().unwrap();
        scr.register_global_module(module.into());
    }
}
