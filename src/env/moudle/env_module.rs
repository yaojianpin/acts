use crate::utils;
use crate::{
    env::{Enviroment, VirtualMachine},
    ActValue, Engine,
};
use rhai::plugin::*;

#[export_module]
mod module {

    pub fn get(vm: &mut VirtualMachine, name: &str) -> Dynamic {
        let v = vm.get(name).unwrap_or(ActValue::Null);
        utils::value_to_dymainc(&v)
    }

    pub fn set<'a>(vm: &mut VirtualMachine, name: &str, value: Dynamic) {
        vm.set(name, utils::dynamic_to_value(&value));
    }
}

impl Enviroment {
    pub fn registry_env_module(&self, _engine: &Engine) {
        let module = rhai::exported_module!(module);
        self.register_global_module(module);
    }
}
