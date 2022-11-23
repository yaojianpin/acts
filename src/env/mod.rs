mod moudle;
#[cfg(test)]
mod tests;
mod vm;

use crate::{debug, ActError, ActModule, ActResult, Engine};
use rhai::Engine as ScriptEngine;
use std::sync::{Arc, Mutex};
pub use vm::VirtualMachine;

#[derive(Default, Clone)]
pub struct Enviroment {
    pub(crate) scr: Arc<Mutex<ScriptEngine>>,
}

unsafe impl Send for Enviroment {}
unsafe impl Sync for Enviroment {}

impl Enviroment {
    pub fn new() -> Self {
        let scr = ScriptEngine::new();
        let env = Enviroment {
            scr: Arc::new(Mutex::new(scr)),
        };

        env
    }

    pub fn vm(&self) -> VirtualMachine {
        VirtualMachine::new(&self)
    }

    pub fn init(&self, engine: &Engine) {
        debug!("env::init");
        self.registry_collection_module(engine);
        self.registry_act_module(engine);
        self.registry_console_module(engine);
        self.registry_env_module(engine);

        let modules = engine.modules();
        for (name, module) in modules.iter() {
            if name.starts_with("::") {
                self.register_global_module(module.clone());
            } else {
                self.register_module(name, module.clone());
            }
        }
    }

    pub fn run(&self, script: &str, vm: &VirtualMachine) -> ActResult<bool> {
        let scr = self.scr.lock().unwrap();
        let mut scope = vm.scope.write().unwrap();

        match scr.run_with_scope(&mut scope, script) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::ScriptError(format!("{}", err))),
        }
    }
    pub fn eval<T: rhai::Variant + Clone>(&self, expr: &str, vm: &VirtualMachine) -> ActResult<T> {
        let scr = self.scr.lock().unwrap();
        let mut scope = vm.scope.write().unwrap();
        match scr.eval_with_scope::<T>(&mut scope, expr) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::ScriptError(format!("{}", err))),
        }
    }

    pub fn act(&self, _act_name: &str) {}

    fn register_module(&self, name: impl AsRef<str>, module: ActModule) {
        let scr = &mut *self.scr.lock().unwrap();
        scr.register_static_module(name, module.into());
    }

    fn register_global_module(&self, module: ActModule) {
        let scr = &mut *self.scr.lock().unwrap();
        scr.register_global_module(module.into());
    }
}
