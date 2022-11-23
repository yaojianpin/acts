use crate::{adapter::Adapter, plugin::ActPlugin, Engine, RoleAdapter};
use once_cell::sync::OnceCell;
use rhai::{combine_with_exported_module, export_module, Module};
use std::sync::Arc;

static ADAPTER: OnceCell<Arc<Adapter>> = OnceCell::new();

#[derive(Clone)]
pub struct RolePlugin;

impl RolePlugin {
    pub fn new() -> Self {
        RolePlugin
    }

    pub fn adapter() -> &'static Arc<Adapter> {
        ADAPTER.get().unwrap()
    }
}

impl ActPlugin for RolePlugin {
    fn on_init(&self, engine: &Engine) {
        if ADAPTER.get().is_some() {
            return;
        }
        let mut module = Module::new();
        combine_with_exported_module!(&mut module, "role", role_module);
        engine.register_module("::role", &module);

        let adapter = engine.adapter();
        let result = ADAPTER.set(adapter);
        if result.is_err() {
            eprintln!("adapter set error");
        }
    }
}

#[export_module]
mod role_module {
    use rhai::plugin::*;

    #[export_fn]
    pub fn role(name: &str) -> Vec<Dynamic> {
        RolePlugin::adapter()
            .role(name)
            .into_iter()
            .map(|u| Dynamic::from(u))
            .collect()
    }
}
