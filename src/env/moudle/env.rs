use crate::env::Enviroment;
use crate::utils;
use rhai::plugin::*;

#[export_module]
mod env_module {

    pub fn get(env: &mut Enviroment, name: &str) -> Dynamic {
        let v = env.get(name).expect(&format!("fail to get env '{name}'"));
        utils::value_to_dymainc(&v)
    }

    pub fn set(env: &mut Enviroment, name: &str, value: Dynamic) {
        env.set(name, utils::dynamic_to_value(&value));
    }
}

#[export_module]
mod vm_module {
    use crate::env::VirtualMachine;

    pub fn get(env: &mut VirtualMachine, name: &str) -> Dynamic {
        if let Some(v) = env.get(name) {
            return utils::value_to_dymainc(&v);
        }

        Dynamic::UNIT
    }

    pub fn set(env: &mut VirtualMachine, name: &str, value: Dynamic) {
        env.set(name, utils::dynamic_to_value(&value));
    }
}

impl Enviroment {
    pub fn registry_env_module(&self) {
        let module = rhai::exported_module!(env_module);
        self.register_global_module(module);

        let module = rhai::exported_module!(vm_module);
        self.register_global_module(module);
    }
}
