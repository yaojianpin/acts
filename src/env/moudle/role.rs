use crate::{env::Enviroment, Candidate};
use rhai::{export_module, plugin::*};

impl Enviroment {
    pub fn registry_role_module(&self) {
        let mut module = Module::new();
        combine_with_exported_module!(&mut module, "role", role_module);
        self.register_global_module(module);
    }
}

#[export_module]
mod role_module {
    use rhai::plugin::*;

    #[export_fn]
    pub fn role(name: &str) -> Vec<Dynamic> {
        vec![Candidate::Role(name.to_string()).into()]
    }
}
