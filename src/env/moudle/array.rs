use crate::{ActError, ActModule, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

#[derive(Clone)]
pub struct Array {}

#[rquickjs::module(rename_vars = "camelCase")]
mod array {
    use std::collections::BTreeSet;

    #[rquickjs::function]
    pub fn intersection(a: Vec<String>, b: Vec<String>) -> Vec<String> {
        let mut set_a = BTreeSet::new();
        let mut set_b = BTreeSet::new();
        for v in a.iter() {
            set_a.insert(v);
        }

        for v in b.iter() {
            set_b.insert(v);
        }

        set_a
            .intersection(&set_b)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
    }

    #[rquickjs::function]
    pub fn union(a: Vec<String>, b: Vec<String>) -> Vec<String> {
        let mut set_a = BTreeSet::new();
        let mut set_b = BTreeSet::new();
        for v in a.iter() {
            set_a.insert(v);
        }

        for v in b.iter() {
            set_b.insert(v);
        }

        set_a
            .union(&set_b)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
    }

    #[rquickjs::function]
    pub fn difference(a: Vec<String>, b: Vec<String>) -> Vec<String> {
        let mut set_a = BTreeSet::new();
        let mut set_b = BTreeSet::new();

        for v in a.iter() {
            set_a.insert(v);
        }

        for v in b.iter() {
            set_b.insert(v);
        }
        set_a
            .difference(&set_b)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
    }
}

impl Array {
    pub fn new() -> Self {
        Self {}
    }
}

impl ActModule for Array {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        JsModule::declare_def::<js_array, _>(ctx.clone(), "@acts/array").unwrap();

        let source = r#"
        import { intersection, union, difference } from '@acts/array';
        Array.prototype.intersection = function(b){
            return intersection(this, b);
        }

        Array.prototype.union = function(b){
            return union(this, b);
        }

        Array.prototype.difference = function(b){
            return difference(this, b);
        }
        "#;

        let _ = JsModule::evaluate(ctx.clone(), "@acts/array", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;
        Ok(())
    }
}
