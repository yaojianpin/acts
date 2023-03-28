use crate::{adapter::Adapter, plugin::ActPlugin, Engine, OrgAdapter};
use once_cell::sync::OnceCell;
use rhai::{combine_with_exported_module, export_module, Module};
use std::sync::Arc;

static ADAPTER: OnceCell<Arc<Adapter>> = OnceCell::new();

#[derive(Debug, Clone)]
pub struct OrgPlugin;

impl OrgPlugin {
    pub fn new() -> Self {
        OrgPlugin
    }

    pub fn adapter() -> Arc<Adapter> {
        ADAPTER.get().unwrap().clone()
    }
}

impl ActPlugin for OrgPlugin {
    fn on_init(&self, engine: &Engine) {
        if ADAPTER.get().is_some() {
            return;
        }
        let mut module = Module::new();
        combine_with_exported_module!(&mut module, "org", org_module);
        engine.extender().register_module("::org", &module);

        let adapter = engine.adapter();
        let result = ADAPTER.set(adapter);
        if result.is_err() {
            eprintln!("adapter set error");
        }
    }
}

#[export_module]
mod org_module {
    use rhai::plugin::*;

    #[export_fn()]
    pub fn user(id: &str) -> Vec<Dynamic> {
        let mut map = rhai::Map::new();
        map.insert("id".into(), Dynamic::from(id.to_string()));
        map.insert("type".into(), Dynamic::from("p"));

        vec![Dynamic::from(map)]
    }

    #[export_fn()]
    pub fn dept(id: &str) -> Vec<Dynamic> {
        let mut map = rhai::Map::new();
        map.insert("id".into(), Dynamic::from(id.to_string()));
        map.insert("type".into(), Dynamic::from("d"));

        vec![Dynamic::from(map)]
    }

    #[export_fn()]
    pub fn unit(id: &str) -> Vec<Dynamic> {
        let mut map = rhai::Map::new();
        map.insert("id".into(), Dynamic::from(id.to_string()));
        map.insert("type".into(), Dynamic::from("u"));

        vec![Dynamic::from(map)]
    }

    #[export_fn]
    pub fn relate(ids: &mut Vec<Dynamic>, r: &str) -> Vec<Dynamic> {
        let mut ret = Vec::new();
        ids.iter().for_each(|d| {
            let map = d.clone().cast::<rhai::Map>();

            let t = map.get("type").unwrap();
            let id = map.get("id").unwrap();

            let adapter = OrgPlugin::adapter();
            let items = adapter
                .relate(&t.clone().cast::<String>(), &id.clone().cast::<String>(), r)
                .clone();
            for item in items {
                ret.push(Dynamic::from(item));
            }
        });

        ret
    }
}
