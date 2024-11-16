mod block;
mod call;
mod catch;
mod chain;
mod r#do;
mod each;
mod r#if;
mod irq;
mod msg;
mod pack;
mod timeout;

use crate::{ModelBase, StmtBuild, Vars};
pub use block::Block;
pub use call::Call;
pub use catch::Catch;
pub use chain::Chain;
pub use each::Each;
pub use irq::Irq;
pub use msg::Msg;
pub use pack::Pack;
pub use r#do::Do;
pub use r#if::If;
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
pub use timeout::{Timeout, TimeoutLimit, TimeoutUnit};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Act {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default, rename = "act")]
    pub act: String,

    /// act key for req and msg
    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub tag: String,

    /// in expression for 'each' and 'chain'
    #[serde(default)]
    pub r#in: String,

    /// on expression for 'if' and 'on_timeout'
    #[serde(default)]
    pub on: String,

    /// act arguments for act fnction, such as 'set', 'req'
    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub rets: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub setup: Vec<Act>,

    /// act list for act event, such as on_completed, on_catch, on_timeout
    #[serde(default)]
    pub then: Vec<Act>,

    // act else for act 'if'
    #[serde(default)]
    pub r#else: Vec<Act>,

    /// next act for 'block'
    #[serde(default)]
    pub next: Option<Box<Act>>,

    #[serde(default)]
    pub catches: Vec<Catch>,

    #[serde(default)]
    pub timeout: Vec<Timeout>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActFn {
    None,

    #[serde(rename = "set")]
    Set(Vars),

    #[serde(rename = "expose")]
    Expose(Vars),

    #[serde(rename = "irq")]
    Irq(Irq),

    #[serde(rename = "msg")]
    Msg(Msg),

    #[serde(rename = "cmd")]
    Cmd(Do),

    #[serde(rename = "each")]
    Each(Each),

    #[serde(rename = "chain")]
    Chain(Chain),

    #[serde(rename = "block")]
    Block(Block),

    #[serde(rename = "if")]
    If(If),

    #[serde(rename = "call")]
    Call(Call),

    #[serde(rename = "pack")]
    Pack(Pack),

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

    #[serde(rename = "on_catch")]
    OnErrorCatch(Vec<Catch>),
}

impl ModelBase for Act {
    fn id(&self) -> &str {
        &self.id
    }
}

impl<T> StmtBuild<T> for Vec<T> {
    fn add(mut self, s: T) -> Self {
        self.push(s);
        self
    }

    fn with<F: Fn(T) -> T>(mut self, build: F) -> Self
    where
        T: Default,
    {
        self.push(build(T::default()));
        self
    }
}

impl Act {
    pub fn is_taskable(&self) -> bool {
        let act_fn = self.into();
        matches!(
            act_fn,
            ActFn::Pack(_) | ActFn::Irq(_) | ActFn::Block(_) | ActFn::Call(_)
        )
    }

    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_act(mut self, act: &str) -> Self {
        self.act = act.to_string();
        self
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_key(mut self, key: &str) -> Self {
        self.key = key.to_string();
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.inputs.set(name, value);
        self
    }

    pub fn with_ret<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.rets.set(name, value);
        self
    }

    pub fn with_then(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.then = build(stmts);
        self
    }

    pub fn with_next<F: Fn(Act) -> Act>(mut self, build: F) -> Self {
        self.next = Some(Box::new(build(Act::default())));
        self
    }

    pub fn with_setup(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.setup = build(stmts);
        self
    }

    pub fn with_in(mut self, expr: &str) -> Self {
        self.r#in = expr.to_string();
        self
    }

    pub fn with_on(mut self, expr: &str) -> Self {
        self.on = expr.to_string();
        self
    }

    pub fn with_catch(mut self, build: fn(Catch) -> Catch) -> Self {
        let catch = Catch::default();
        self.catches.push(build(catch));
        self
    }

    pub fn with_timeout(mut self, build: fn(Timeout) -> Timeout) -> Self {
        let timeout = Timeout::default();
        self.timeout.push(build(timeout));
        self
    }

    pub fn set(var: Vars) -> Self {
        Act {
            inputs: var,
            act: "set".to_string(),
            ..Default::default()
        }
    }

    pub fn expose(var: Vars) -> Self {
        Act {
            inputs: var,
            act: "expose".to_string(),
            ..Default::default()
        }
    }

    pub fn irq<F: Fn(Irq) -> Irq>(build: F) -> Self {
        let req = build(Irq::default());
        Act {
            act: "irq".to_string(),
            inputs: req.inputs,
            tag: req.tag,
            key: req.key,
            rets: req.rets,
            outputs: req.outputs,
            ..Default::default()
        }
    }

    pub fn msg<F: Fn(Msg) -> Msg>(build: F) -> Self {
        let msg = build(Msg::default());
        Act {
            inputs: msg.inputs,
            tag: msg.tag,
            key: msg.key,
            act: "msg".to_string(),
            ..Default::default()
        }
    }

    pub fn r#if<F: Fn(If) -> If>(build: F) -> Self {
        let cond = build(If::default());
        Act {
            on: cond.on,
            then: cond.then,
            r#else: cond.r#else,
            act: "if".to_string(),
            ..Default::default()
        }
    }

    pub fn each<F: Fn(Each) -> Each>(build: F) -> Self {
        let each = build(Each::default());
        Act {
            r#in: each.r#in,
            then: each.then,
            act: "each".to_string(),
            ..Default::default()
        }
    }

    pub fn call<F: Fn(Call) -> Call>(build: F) -> Self {
        let call = build(Call::default());
        Act {
            key: call.key,
            inputs: call.inputs,
            rets: call.rets,
            act: "call".to_string(),
            ..Default::default()
        }
    }

    pub fn chain<F: Fn(Chain) -> Chain>(build: F) -> Self {
        let chain = build(Chain::default());
        Act {
            r#in: chain.r#in,
            then: chain.then,
            act: "chain".to_string(),
            ..Default::default()
        }
    }

    pub fn cmd<F: Fn(Do) -> Do>(build: F) -> Self {
        let cmd = build(Do::default());
        Act {
            inputs: cmd.inputs,
            key: cmd.key,
            act: "cmd".to_string(),
            ..Default::default()
        }
    }

    pub fn block<F: Fn(Block) -> Block>(build: F) -> Self {
        let block = build(Block::default());
        Act {
            inputs: block.inputs,
            then: block.then,
            next: block.next,
            act: "block".to_string(),
            ..Default::default()
        }
    }

    pub fn pack<F: Fn(Pack) -> Pack>(build: F) -> Self {
        let pack = build(Pack::default());

        Act {
            act: "pack".to_string(),
            key: pack.key,
            inputs: pack.inputs,
            outputs: pack.outputs,
            ..Default::default()
        }
    }

    pub fn catch<F: Fn(Catch) -> Catch>(build: F) -> Self {
        let c = build(Catch::default());

        Act {
            act: "catch".to_string(),
            on: match c.on {
                Some(on) => on,
                None => "".to_string(),
            },
            inputs: c.inputs,
            then: c.then,
            ..Default::default()
        }
    }

    pub fn timeout<F: Fn(Timeout) -> Timeout>(build: F) -> Self {
        let timeout = build(Timeout::default());

        Act {
            act: "timeout".to_string(),
            on: timeout.on.to_string(),
            then: timeout.then,
            ..Default::default()
        }
    }

    pub fn on_created(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_created".to_string(),
            then: stmts,
            ..Default::default()
        }
    }

    pub fn on_completed(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_completed".to_string(),
            then: stmts,
            ..Default::default()
        }
    }

    pub fn on_before_update(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_before_update".to_string(),
            then: stmts,
            ..Default::default()
        }
    }

    pub fn on_updated(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_updated".to_string(),
            then: stmts,
            ..Default::default()
        }
    }

    pub fn on_step(build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_step".to_string(),
            then: stmts,
            ..Default::default()
        }
    }

    pub fn on_catch(build: fn(Vec<Catch>) -> Vec<Catch>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_catch".to_string(),
            catches: stmts,
            ..Default::default()
        }
    }

    pub fn on_timeout(build: fn(Vec<Timeout>) -> Vec<Timeout>) -> Self {
        let stmts = build(Vec::new());
        Act {
            act: "on_timeout".to_string(),
            timeout: stmts,
            ..Default::default()
        }
    }
}

impl From<&Act> for ActFn {
    fn from(act: &Act) -> Self {
        let fn_name = act.act.as_str();
        match fn_name {
            "set" => ActFn::Set(act.inputs.clone()),
            "expose" => ActFn::Expose(act.inputs.clone()),
            "irq" => {
                let irq = Irq {
                    tag: act.tag.clone(),
                    key: act.key.clone(),
                    inputs: act.inputs.clone(),
                    rets: act.rets.clone(),
                    ..Default::default()
                };
                ActFn::Irq(irq)
            }
            "msg" => {
                let msg = Msg {
                    tag: act.tag.clone(),
                    key: act.key.clone(),
                    inputs: act.inputs.clone(),
                };
                ActFn::Msg(msg)
            }
            "cmd" => {
                let cmd = Do {
                    key: act.key.clone(),
                    inputs: act.inputs.clone(),
                };
                ActFn::Cmd(cmd)
            }
            "each" => {
                let each = Each {
                    r#in: act.r#in.clone(),
                    then: act.then.clone(),
                };
                ActFn::Each(each)
            }
            "chain" => {
                let chain = Chain {
                    r#in: act.r#in.clone(),
                    then: act.then.clone(),
                };
                ActFn::Chain(chain)
            }
            "block" => {
                let block = Block {
                    then: act.then.clone(),
                    inputs: act.inputs.clone(),
                    next: act.next.clone(),
                };
                ActFn::Block(block)
            }
            "if" => {
                let r#if = If {
                    on: act.on.clone(),
                    then: act.then.clone(),
                    r#else: act.r#else.clone(),
                };
                ActFn::If(r#if)
            }
            "call" => {
                let call = Call {
                    key: act.key.clone(),
                    inputs: act.inputs.clone(),
                    rets: act.rets.clone(),
                };
                ActFn::Call(call)
            }
            "pack" => {
                let pack = Pack {
                    key: act.key.clone(),
                    inputs: act.inputs.clone(),
                    outputs: act.rets.clone(),
                };
                ActFn::Pack(pack)
            }
            "on_created" => ActFn::OnCreated(act.then.clone()),
            "on_timeout" => ActFn::OnTimeout(act.timeout.clone()),
            "on_updated" => ActFn::OnUpdated(act.then.clone()),
            "on_before_update" => ActFn::OnBeforeUpdate(act.then.clone()),
            "on_step" => ActFn::OnStep(act.then.clone()),
            "on_completed" => ActFn::OnCompleted(act.then.clone()),
            "on_catch" => ActFn::OnErrorCatch(act.catches.clone()),
            _ => ActFn::None,
        }
    }
}
