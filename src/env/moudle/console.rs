use crate::env::Enviroment;
use rhai::{export_module, plugin::*};

impl Enviroment {
    pub fn registry_console_module(&self) {
        let mut module = Module::new();
        combine_with_exported_module!(&mut module, "console", console);
        self.register_module("console", module)
    }
}

#[export_module]
mod console {
    use rhai::plugin::*;

    #[export_fn]
    pub fn log(message: &str) {
        println!("{}", message);
    }

    #[export_fn]
    pub fn dbg(_message: &str) {
        println!("{}", format!("[debug]{}", _message));
    }

    #[export_fn]
    pub fn info(_message: &str) {
        println!("{}", format!("[info]{}", _message));
    }

    #[export_fn]
    pub fn wran(_message: &str) {
        println!("{}", format!("[wran]{}", _message));
    }

    #[export_fn]
    pub fn error(_message: &str) {
        println!("{}", format!("[error]{}", _message));
    }
}
