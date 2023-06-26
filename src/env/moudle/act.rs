use crate::env::Enviroment;
use rhai::{export_module, plugin::*};

impl Enviroment {
    pub fn registry_act_module(&self) {
        let module = rhai::exported_module!(act_module);
        self.register_global_module(module);
    }
}

#[export_module]
mod act_module {
    use crate::{Candidate, Context};

    #[export_fn]
    pub fn send(ctx: &mut Context, key: &str) {
        ctx.dispatch_message(key);
    }

    pub fn end(ctx: &mut Context) {
        println!("end {}", ctx.task.node.data().name());
    }

    pub fn abort(ctx: &mut Context) {
        println!("abort {}", ctx.task.node.data().name());
    }

    #[export_fn]
    pub fn role(_ctx: &mut Context, name: &str) -> Vec<Dynamic> {
        vec![Candidate::Role(name.to_string()).into()]
    }

    #[export_fn()]
    pub fn user(_ctx: &mut Context, id: &str) -> Dynamic {
        Candidate::User(id.to_string()).into()
    }

    #[export_fn()]
    pub fn dept(_ctx: &mut Context, id: &str) -> Dynamic {
        Candidate::Dept(id.to_string()).into()
    }

    #[export_fn()]
    pub fn unit(_ctx: &mut Context, id: &str) -> Dynamic {
        Candidate::Unit(id.to_string()).into()
    }

    #[export_fn]
    pub fn relate(id: &mut Dynamic, rel: &str) -> Dynamic {
        Candidate::Relation {
            id: id.to_string(),
            rel: rel.to_string(),
        }
        .into()
    }
}
