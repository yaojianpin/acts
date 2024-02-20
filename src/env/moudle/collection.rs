use crate::{
    env::Enviroment,
    model::{Candidate, Operation},
};
use rhai::{export_module, plugin::*};

impl Enviroment {
    pub fn registry_collection_module(&self) {
        let mut module = Module::new();
        combine_with_exported_module!(&mut module, "collection", collection);
        self.register_global_module(module)
    }
}

#[export_module]
mod collection {
    use rhai::plugin::*;

    #[export_fn]
    pub fn intersect(a: Dynamic, b: Dynamic) -> Dynamic {
        let mut result_a = Vec::new();
        if a.is_array() {
            result_a.extend(a.into_array().unwrap());
        } else {
            result_a.push(a);
        }

        let mut result_b = Vec::new();
        if b.is_array() {
            result_b.extend(b.into_array().unwrap());
        } else {
            result_b.push(b);
        }
        let a = Candidate::Set(
            result_a
                .iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        let b = Candidate::Set(
            result_b
                .iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        Candidate::Group {
            op: Operation::Intersect,
            items: vec![a, b],
        }
        .into()
    }

    #[export_fn]
    pub fn union(a: Dynamic, b: Dynamic) -> Dynamic {
        let mut result = Vec::new();
        if a.is_array() {
            result.extend(a.into_array().unwrap());
        } else {
            result.push(a);
        }

        if b.is_array() {
            result.extend(b.into_array().unwrap());
        } else {
            result.push(b);
        }

        Candidate::Group {
            op: Operation::Union,
            items: result
                .iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        }
        .into()
    }

    #[export_fn]
    pub fn except(a: Dynamic, b: Dynamic) -> Dynamic {
        let mut result_a = Vec::new();
        if a.is_array() {
            result_a.extend(a.into_array().unwrap());
        } else {
            result_a.push(a);
        }

        let mut result_b = Vec::new();
        if b.is_array() {
            result_b.extend(b.into_array().unwrap());
        } else {
            result_b.push(b);
        }

        let a = Candidate::Set(
            result_a
                .iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        let b = Candidate::Set(
            result_b
                .iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        Candidate::Group {
            op: Operation::Except,
            items: vec![a, b],
        }
        .into()
    }
}
