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
    pub fn intersect(a: Vec<Dynamic>, b: Vec<Dynamic>) -> Vec<Dynamic> {
        let a = Candidate::Set(
            a.iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        let b = Candidate::Set(
            b.iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        vec![Candidate::Group {
            op: Operation::Intersect,
            items: vec![a, b],
        }
        .into()]
    }

    #[export_fn]
    pub fn union(a: Vec<Dynamic>, b: Vec<Dynamic>) -> Vec<Dynamic> {
        let mut result = a.clone();
        result.extend(b);

        vec![Candidate::Group {
            op: Operation::Union,
            items: result
                .iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        }
        .into()]
    }

    #[export_fn]
    pub fn diff(a: Vec<Dynamic>, b: Vec<Dynamic>) -> Vec<Dynamic> {
        let a = Candidate::Set(
            a.iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        let b = Candidate::Set(
            b.iter()
                .map(|i| Candidate::parse(&i.to_string()).unwrap())
                .collect(),
        );

        vec![Candidate::Group {
            op: Operation::Difference,
            items: vec![a, b],
        }
        .into()]
    }
}
