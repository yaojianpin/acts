mod moudle;
mod vm;

#[cfg(test)]
mod tests;

use crate::{ActError, ActModule, ActResult, ActValue, ShareLock, Vars};
use rhai::Engine as ScriptEngine;
use std::sync::{Arc, Mutex, RwLock};
use tracing::debug;
pub use vm::VirtualMachine;

#[derive(Debug, Default, Clone)]
pub struct Enviroment {
    pub(crate) scr: Arc<Mutex<ScriptEngine>>,
    pub(crate) scope: ShareLock<rhai::Scope<'static>>,
    pub(crate) vars: ShareLock<Vars>,
}

unsafe impl Send for Enviroment {}
unsafe impl Sync for Enviroment {}

impl Enviroment {
    pub fn new() -> Self {
        let scr = ScriptEngine::new();
        let scope = rhai::Scope::new();
        let env = Enviroment {
            scr: Arc::new(Mutex::new(scr)),
            scope: Arc::new(RwLock::new(scope)),
            vars: Arc::new(RwLock::new(Vars::new())),
        };

        env.init();
        env
    }

    pub fn vm(&self) -> Arc<VirtualMachine> {
        // let vars = self.vars();
        let vm = VirtualMachine::new(&self);
        // vm.append(&vars);
        Arc::new(vm)
    }

    pub fn init(&self) {
        debug!("env::init");
        self.registry_collection_module();
        self.registry_console_module();
        self.registry_env_module();
        self.registry_act_module();

        self.scope.write().unwrap().set_or_push("env", self.clone());
    }

    pub fn vars(&self) -> Vars {
        self.vars.read().unwrap().clone()
    }

    pub fn set_scope_var<T: Send + Sync + Clone + 'static>(&self, name: &str, v: &T) {
        self.scope.write().unwrap().set_or_push(name, v.clone());
    }

    pub fn append(&self, vars: &Vars) {
        let env = &mut self.vars.write().unwrap();
        for (name, v) in vars {
            env.entry(name.to_string())
                .and_modify(|i| *i = v.clone())
                .or_insert(v.clone());
        }
    }

    pub fn run_vm(&self, script: &str, vm: &VirtualMachine) -> ActResult<bool> {
        let scr = self.scr.lock().unwrap();
        let mut scope = vm.scope.write().unwrap();
        match scr.run_with_scope(&mut scope, script) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn eval_vm<T: rhai::Variant + Clone>(
        &self,
        expr: &str,
        vm: &VirtualMachine,
    ) -> ActResult<T> {
        let scr = self.scr.lock().unwrap();
        let mut scope = vm.scope.write().unwrap();

        match scr.eval_with_scope::<T>(&mut scope, expr) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn run(&self, script: &str) -> ActResult<bool> {
        let scr = self.scr.lock().unwrap();
        let mut scope = self.scope.write().unwrap();
        match scr.run_with_scope(&mut scope, script) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn eval<T: rhai::Variant + Clone>(&self, expr: &str) -> ActResult<T> {
        let scr = self.scr.lock().unwrap();
        let mut scope = self.scope.write().unwrap();
        match scr.eval_with_scope::<T>(&mut scope, expr) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn get(&self, name: &str) -> Option<ActValue> {
        let vars = self.vars.read().unwrap();
        match vars.get(name) {
            Some(v) => Some(v.clone()),
            None => None,
        }
    }

    pub fn set(&self, name: &str, value: ActValue) {
        let mut vars = self.vars.write().unwrap();
        vars.entry(name.to_string())
            .and_modify(|i| *i = value.clone())
            .or_insert(value);
    }

    pub fn remove(&self, name: &str) {
        let mut vars = self.vars.write().unwrap();
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
