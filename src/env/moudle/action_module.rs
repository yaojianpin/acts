use crate::{env::Enviroment, Engine};

impl Enviroment {
    pub fn registry_act_module(&self, engine: &Engine) {
        let module = engine.action();
        self.register_module("act", module)
    }
}
