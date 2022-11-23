use crate::{env::Enviroment, Engine};
use rhai::{export_module, plugin::*};

impl Enviroment {
    pub fn registry_collection_module(&self, _engine: &Engine) {
        let mut module = Module::new();
        combine_with_exported_module!(&mut module, "g", collection);
        self.register_global_module(module)
    }
}

#[export_module]
mod collection {
    use rhai::plugin::*;

    #[export_fn]
    pub fn intersect(a: Vec<Dynamic>, b: Vec<Dynamic>) -> Vec<String> {
        let b = b
            .iter()
            .map(|i| i.clone().to_string())
            .collect::<Vec<String>>();
        let a = a
            .iter()
            .map(|i| i.clone().to_string())
            .collect::<Vec<String>>();

        let ret = a
            .iter()
            .filter(|i| b.contains(i))
            .map(|i| i.clone())
            .collect::<Vec<_>>();

        ret
    }

    #[export_fn]
    pub fn union(a: Vec<Dynamic>, b: Vec<Dynamic>) -> rhai::Array {
        let b = b
            .iter()
            .map(|i| i.clone().to_string())
            .collect::<Vec<String>>();
        let a = a
            .iter()
            .map(|i| i.clone().to_string())
            .collect::<Vec<String>>();

        let ret = a
            .iter()
            .filter(|i| !b.contains(i))
            .chain(&b)
            .map(|i| Dynamic::from(i.clone()))
            .collect::<Vec<_>>();
        ret
    }

    #[export_fn]
    pub fn minus(a: Vec<Dynamic>, b: Vec<Dynamic>) -> Vec<String> {
        let b = b
            .iter()
            .map(|i| i.clone().to_string())
            .collect::<Vec<String>>();
        let a = a
            .iter()
            .map(|i| i.clone().to_string())
            .collect::<Vec<String>>();

        let ret: Vec<String> = a
            .iter()
            .filter(|i| !b.contains(i))
            .map(|i| i.to_string())
            .collect::<Vec<_>>();

        ret
    }
}
