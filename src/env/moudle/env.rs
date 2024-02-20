use crate::env::Enviroment;
use crate::Vars;
use rhai::plugin::*;

#[export_module]
mod env_module {
    use crate::{utils, Context};

    #[export_fn]
    pub fn get(ctx: &mut Context, key: &str) -> Dynamic {
        if let Some(v) = ctx.task.env().get_env(key) {
            return utils::value_to_dymainc(&v);
        }

        Dynamic::UNIT
    }

    #[export_fn]
    pub fn set(ctx: &mut Context, name: &str, value: Dynamic) {
        ctx.task
            .env()
            .set_env(&Vars::new().with(name, utils::dynamic_to_value(&value)));
    }
}

impl Enviroment {
    pub fn registry_env_module(&self) {
        let module = rhai::exported_module!(env_module);
        self.register_global_module(module);
    }
}
