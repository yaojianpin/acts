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
        ctx.send_message(key);
    }

    pub fn end(ctx: &mut Context) {
        println!("end {}", ctx.task.node.data().name());
    }

    pub fn abort(ctx: &mut Context) {
        println!("abort {}", ctx.task.node.data().name());
    }

    #[export_fn]
    pub fn role(_ctx: &mut Context, v: &str) -> Dynamic {
        Candidate::Role(v.to_string()).into()
    }

    #[export_fn()]
    pub fn user(_ctx: &mut Context, v: &str) -> Dynamic {
        Candidate::User(v.to_string()).into()
    }

    #[export_fn()]
    pub fn dept(_ctx: &mut Context, v: &str) -> Dynamic {
        Candidate::Dept(v.to_string()).into()
    }

    #[export_fn()]
    pub fn unit(_ctx: &mut Context, v: &str) -> Dynamic {
        Candidate::Unit(v.to_string()).into()
    }

    #[export_fn]
    pub fn relate(_ctx: &mut Context, v: &str) -> Dynamic {
        Candidate::Relation(v.to_string()).into()
    }
}
