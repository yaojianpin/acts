mod chain;
mod cmd;
mod each;
mod r#if;
mod msg;
mod pack;
mod req;
mod r#use;

use crate::{Catch, ModelBase, StmtBuild, Timeout, Vars};
pub use chain::Chain;
pub use cmd::Cmd;
pub use each::Each;
pub use msg::Msg;
pub use pack::Package;
pub use r#if::If;
pub use r#use::Use;
pub use req::Req;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Act {
    #[serde(rename = "set")]
    Set(Vars),

    #[serde(rename = "expose")]
    Expose(Vars),

    #[serde(rename = "req")]
    Req(Req),

    #[serde(rename = "msg")]
    Msg(Msg),

    #[serde(rename = "cmd")]
    Cmd(Cmd),

    #[serde(rename = "each")]
    Each(Each),

    #[serde(rename = "chain")]
    Chain(Chain),

    #[serde(rename = "pack")]
    Package(Package),

    #[serde(rename = "if")]
    If(If),

    #[serde(rename = "use")]
    Use(Use),

    #[serde(rename = "on_created")]
    OnCreated(Vec<Act>),

    #[serde(rename = "on_timeout")]
    OnTimeout(Vec<Timeout>),

    #[serde(rename = "on_updated")]
    OnUpdated(Vec<Act>),

    #[serde(rename = "on_before_update")]
    OnBeforeUpdate(Vec<Act>),

    #[serde(rename = "on_step")]
    OnStep(Vec<Act>),

    #[serde(rename = "on_completed")]
    OnCompleted(Vec<Act>),

    #[serde(rename = "on_error_catch")]
    OnErrorCatch(Vec<Catch>),
}

impl ModelBase for Act {
    fn id(&self) -> &str {
        match self {
            Act::Req(req) => &req.id,
            Act::Msg(msg) => &msg.id,
            Act::Use(r#use) => &r#use.id,
            Act::Package(pack) => &pack.id,
            _ => "",
        }
    }
}

impl<T> StmtBuild<T> for Vec<T> {
    fn add(mut self, s: T) -> Self {
        self.push(s);
        self
    }
}

impl Act {
    pub fn kind(&self) -> &str {
        match self {
            Act::Set(_) => "set",
            Act::Expose(_) => "expose",
            Act::Req(_) => "req",
            Act::Msg(_) => "msg",
            Act::Cmd(_) => "cmd",
            Act::Each(_) => "each",
            Act::Chain(_) => "chain",
            Act::Package(_) => "pack",
            Act::If(_) => "if",
            Act::Use(_) => "use",
            Act::OnCreated(_) => "on_created",
            Act::OnTimeout(_) => "on_timeout",
            Act::OnBeforeUpdate(_) => "on_before_update",
            Act::OnUpdated(_) => "on_updated",
            Act::OnStep(_) => "on_step",
            Act::OnCompleted(_) => "on_completed",
            Act::OnErrorCatch(_) => "on_error_catch",
        }
    }

    pub fn is_taskable(&self) -> bool {
        match self {
            Act::Req(_) | Act::Package(_) | Act::Use(_) => true,
            _ => false,
        }
    }

    pub fn set_id(&mut self, id: &str) {
        match self {
            Act::Req(req) => req.id = id.to_string(),
            Act::Msg(msg) => msg.id = id.to_string(),
            Act::Use(r#use) => r#use.id = id.to_string(),
            Act::Package(pack) => pack.id = id.to_string(),
            _ => {}
        }
    }

    pub fn tag(&self) -> &str {
        match self {
            Act::Req(req) => &req.tag,
            Act::Msg(msg) => &msg.tag,
            _ => "",
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Act::Req(req) => &req.key,
            Act::Msg(msg) => &msg.key,
            _ => "",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Act::Req(req) => &req.name,
            Act::Msg(msg) => &msg.name,
            Act::Cmd(cmd) => &cmd.name,
            _ => "",
        }
    }

    pub fn inputs(&self) -> Vars {
        match self {
            Act::Req(req) => req.inputs.clone(),
            Act::Msg(msg) => msg.inputs.clone(),
            Act::Use(r#use) => r#use.inputs.clone(),
            _ => Vars::new(),
        }
    }

    pub fn outputs(&self) -> Vars {
        match self {
            Act::Req(req) => req.outputs.clone(),
            Act::Use(u) => u.outputs.clone(),
            _ => Vars::new(),
        }
    }

    pub fn rets(&self) -> Vars {
        match self {
            Act::Req(req) => req.rets.clone(),
            _ => Vars::new(),
        }
    }

    pub fn set(var: Vars) -> Self {
        Act::Set(var)
    }

    pub fn expose(var: Vars) -> Self {
        Act::Expose(var)
    }

    pub fn req(build: fn(Req) -> Req) -> Self {
        let req = Req::default();
        Act::Req(build(req))
    }

    pub fn msg(build: fn(Msg) -> Msg) -> Self {
        let msg = Msg::default();
        Act::Msg(build(msg))
    }

    pub fn r#if(build: fn(If) -> If) -> Self {
        let cond = If::default();
        Act::If(build(cond))
    }

    pub fn each(build: fn(Each) -> Each) -> Self {
        let each = Each::default();
        Act::Each(build(each))
    }

    pub fn r#use(build: fn(Use) -> Use) -> Self {
        let u = Use::default();
        Act::Use(build(u))
    }

    pub fn chain(build: fn(Chain) -> Chain) -> Self {
        let chain = Chain::default();
        Act::Chain(build(chain))
    }

    pub fn cmd(build: fn(Cmd) -> Cmd) -> Self {
        let cmd = Cmd::default();
        Act::Cmd(build(cmd))
    }

    pub fn pack(build: fn(Package) -> Package) -> Self {
        let pack = Package::default();
        Act::Package(build(pack))
    }

    pub fn on_created(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        Act::OnCreated(build(stmts))
    }

    pub fn on_completed(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        Act::OnCompleted(build(stmts))
    }

    pub fn on_before_update(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        Act::OnBeforeUpdate(build(stmts))
    }

    pub fn on_updated(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        Act::OnUpdated(build(stmts))
    }

    pub fn on_step(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        Act::OnStep(build(stmts))
    }

    pub fn on_error_catch(build: fn(Vec<Catch>) -> Vec<Catch>) -> Self {
        let stmts = Vec::new();
        Act::OnErrorCatch(build(stmts))
    }

    pub fn on_timeout(build: fn(Vec<Timeout>) -> Vec<Timeout>) -> Self {
        let stmts = Vec::new();
        Act::OnTimeout(build(stmts))
    }
}
