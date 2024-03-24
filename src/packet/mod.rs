bindgen!({ path: "src/packet/pack.wit", world: "pack", async: true });

use self::acts::packs::{act, log, types};
use crate::{utils::consts, Act, Action, Block, Call, Chain, Context, Each, Msg, Req, Vars};
use async_trait::async_trait;
use serde_json::json;
use wasmtime::component::bindgen;

pub struct ActComponent<'a> {
    ctx: &'a Context,
}

pub struct LogComponent;

impl<'a> ActComponent<'a> {
    pub fn new(ctx: &'a Context) -> Self {
        Self { ctx }
    }
}

impl LogComponent {
    pub fn new() -> Self {
        Self
    }
}
#[async_trait]
impl log::Host for LogComponent {
    async fn info(&mut self, message: String) -> wasmtime::Result<()> {
        println!("[info] {}", message);
        Ok(())
    }

    async fn error(&mut self, message: String) -> wasmtime::Result<()> {
        println!("[error] {}", message);
        Ok(())
    }

    async fn warn(&mut self, message: String) -> wasmtime::Result<()> {
        println!("[warn] {}", message);
        Ok(())
    }
}

#[async_trait]
impl act::Host for ActComponent<'_> {
    async fn inputs(&mut self) -> wasmtime::Result<Vec<(String, types::Value)>> {
        Ok(self
            .ctx
            .task
            .inputs()
            .iter()
            .map(|(k, v)| (k.to_string(), v.into()))
            .collect::<Vec<_>>())
    }

    async fn set_data(&mut self, key: String, value: act::Value) -> wasmtime::Result<()> {
        let v: serde_json::Value = value.into();
        self.ctx.task.env().set_env(&Vars::new().with(&key, v));
        Ok(())
    }

    async fn set_output(&mut self, key: String, value: act::Value) -> wasmtime::Result<()> {
        let outputs: Vars = self
            .ctx
            .task
            .env()
            .get(consts::ACT_OUTPUTS)
            .unwrap_or_default();

        let v: serde_json::Value = value.into();
        self.ctx
            .task
            .env()
            .set(consts::ACT_OUTPUTS, outputs.with(&key, v));
        Ok(())
    }

    async fn complete(&mut self) -> wasmtime::Result<()> {
        self.ctx.scher.do_action(&Action::new(
            &self.ctx.task.proc_id,
            &self.ctx.task.id,
            consts::EVT_COMPLETE,
            &Vars::new(),
        ))?;
        Ok(())
    }

    async fn abort(&mut self) -> wasmtime::Result<()> {
        let ctx = self.ctx.task.create_context(&self.ctx.scher);
        ctx.set_action(&Action::new(
            &self.ctx.task.proc_id,
            &self.ctx.task.id,
            consts::EVT_ABORT,
            &Vars::new(),
        ))?;
        self.ctx.task.update(&ctx)?;

        Ok(())
    }

    async fn back(&mut self, nid: String) -> wasmtime::Result<()> {
        let ctx = self.ctx.task.create_context(&self.ctx.scher);
        ctx.set_action(&Action::new(
            &self.ctx.task.proc_id,
            &self.ctx.task.id,
            consts::EVT_BACK,
            &Vars::new().with(consts::ACT_TO, nid),
        ))?;
        self.ctx.task.update(&ctx)?;

        Ok(())
    }

    async fn skip(&mut self) -> wasmtime::Result<()> {
        let ctx = self.ctx.task.create_context(&self.ctx.scher);
        ctx.set_action(&Action::new(
            &self.ctx.task.proc_id,
            &self.ctx.task.id,
            consts::EVT_SKIP,
            &Vars::new(),
        ))?;
        self.ctx.task.update(&ctx)?;

        Ok(())
    }

    async fn fail(&mut self, ecode: String, message: String) -> wasmtime::Result<()> {
        let ctx = self.ctx.task.create_context(&self.ctx.scher);
        ctx.set_action(&Action::new(
            &self.ctx.task.proc_id,
            &self.ctx.task.id,
            consts::EVT_BACK,
            &Vars::new()
                .with(consts::ACT_ERR_CODE, ecode)
                .with(consts::ACT_ERR_MESSAGE, message),
        ))?;
        self.ctx.task.update(&ctx)?;

        Ok(())
    }

    async fn push(&mut self, p: act::Packet) -> wasmtime::Result<()> {
        let act: Act = p.into();
        act.exec(self.ctx)?;
        Ok(())
    }

    async fn push_req(
        &mut self,
        req: types::Request,
        events: types::RequestEvents,
    ) -> wasmtime::Result<()> {
        let mut req: Req = req.into();

        if events.on_created.len() > 0 {
            let mut evts = Vec::new();
            for e in events.on_created {
                evts.push(e.into());
            }
            req.on_created = evts;
        }

        if events.on_completed.len() > 0 {
            let mut evts = Vec::new();
            for e in events.on_completed {
                evts.push(e.into());
            }
            req.on_completed = evts;
        }

        let act = Act::Req(req);
        act.exec(self.ctx)?;

        Ok(())
    }

    async fn push_msg(&mut self, m: types::Message) -> wasmtime::Result<()> {
        let act = Act::Msg(Msg {
            id: m.id,
            name: m.name.unwrap_or_default(),
            tag: m.tag.unwrap_or_default(),
            key: m.key.unwrap_or_default(),
            inputs: m.inputs.into(),
            ..Default::default()
        });
        act.exec(self.ctx)?;
        Ok(())
    }

    async fn push_chain(&mut self, c: types::Chain) -> wasmtime::Result<()> {
        let mut ins = Vec::new();
        for i in c.ins {
            ins.push(json!(i));
        }

        let mut stmts = Vec::new();
        stmts.push(Act::req(|r| r.with_key("")));
        let act = Act::Chain(Chain {
            r#in: serde_json::to_string(&ins).unwrap_or_default(),
            run: stmts,
            ..Default::default()
        });

        act.exec(self.ctx)?;
        Ok(())
    }

    async fn push_each(&mut self, e: types::Each) -> wasmtime::Result<()> {
        let mut ins = Vec::new();
        for i in e.ins {
            ins.push(json!(i));
        }

        let mut stmts = Vec::new();
        stmts.push(Act::req(|r| r.with_key("")));
        let act = Act::Each(Each {
            r#in: serde_json::to_string(&ins).unwrap_or_default(),
            run: stmts,
            ..Default::default()
        });

        act.exec(self.ctx)?;
        Ok(())
    }

    async fn push_block(
        &mut self,
        block: Vec<types::Packet>,
        next: Option<Vec<types::Packet>>,
    ) -> wasmtime::Result<()> {
        let mut acts = Vec::new();
        for b in block {
            acts.push(b.into());
        }

        let mut next_acts = Vec::new();
        if let Some(n) = next {
            for b in n {
                next_acts.push(b.into());
            }
        }

        let act = Act::Block(Block {
            acts,
            next: Some(Box::new(Block {
                acts: next_acts,
                ..Default::default()
            })),
            ..Default::default()
        });
        act.exec(self.ctx)?;
        Ok(())
    }

    async fn push_call(&mut self, u: types::Call) -> wasmtime::Result<()> {
        let act = Act::Call(Call {
            mid: u.mid,
            inputs: u.inputs.into(),
            outputs: u.outputs.into(),
            ..Default::default()
        });
        act.exec(self.ctx)?;
        Ok(())
    }
}

impl From<act::Value> for serde_json::Value {
    fn from(value: act::Value) -> Self {
        match value {
            act::Value::Null => serde_json::Value::Null,
            act::Value::Boolean(v) => json!(v),
            act::Value::PosInt(v) => json!(v),
            act::Value::NegInt(v) => json!(v),
            act::Value::Float(v) => json!(v),
            act::Value::Text(v) => json!(v),
        }
    }
}

impl From<Vec<(String, act::Value)>> for Vars {
    fn from(value: Vec<(String, act::Value)>) -> Self {
        let mut vars = Vars::new();
        for (key, v) in value {
            let v: serde_json::Value = v.into();
            vars.set(&key, v);
        }
        vars
    }
}

impl From<&serde_json::Value> for act::Value {
    fn from(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => act::Value::Null,
            serde_json::Value::Bool(v) => act::Value::Boolean(v.clone()),
            serde_json::Value::Number(v) => {
                if v.is_u64() {
                    return act::Value::PosInt(v.as_u64().unwrap());
                } else if v.is_i64() {
                    return act::Value::NegInt(v.as_i64().unwrap());
                } else {
                    return act::Value::Float(v.as_f64().unwrap());
                }
            }
            serde_json::Value::String(v) => act::Value::Text(v.clone()),
            serde_json::Value::Array(v) => act::Value::Text(serde_json::to_string(v).unwrap()),
            serde_json::Value::Object(v) => act::Value::Text(serde_json::to_string(v).unwrap()),
        }
    }
}

impl From<types::Packet> for Act {
    fn from(pack: types::Packet) -> Self {
        match pack {
            types::Packet::Req(r) => Act::Req(Req {
                id: r.id,
                name: r.name.unwrap_or_default(),
                tag: r.tag.unwrap_or_default(),
                key: r.key.unwrap_or_default(),
                inputs: r.inputs.into(),
                outputs: r.outputs.into(),
                ..Default::default()
            }),
            types::Packet::Msg(m) => Act::Msg(Msg {
                id: m.id,
                name: m.name.unwrap_or_default(),
                tag: m.tag.unwrap_or_default(),
                key: m.key.unwrap_or_default(),
                inputs: m.inputs.into(),
                ..Default::default()
            }),
            types::Packet::Chain(c) => {
                let mut ins = Vec::new();
                for i in c.ins {
                    ins.push(json!(i));
                }

                let mut stmts = Vec::new();
                stmts.push(Act::req(|r| r.with_key("")));
                let act = Act::Chain(Chain {
                    r#in: serde_json::to_string(&ins).unwrap_or_default(),
                    run: stmts,
                    ..Default::default()
                });

                act
            }
            types::Packet::Each(c) => {
                let mut ins = Vec::new();
                for i in c.ins {
                    ins.push(json!(i));
                }

                let mut stmts = Vec::new();
                stmts.push(Act::req(|r| r.with_key("")));
                let act = Act::Each(Each {
                    r#in: serde_json::to_string(&ins).unwrap_or_default(),
                    run: stmts,
                    ..Default::default()
                });

                act
            }
            types::Packet::Call(c) => Act::Call(Call {
                mid: c.mid,
                inputs: c.inputs.into(),
                outputs: c.outputs.into(),
                ..Default::default()
            }),
        }
    }
}

impl From<types::Request> for Req {
    fn from(r: types::Request) -> Self {
        Req {
            id: r.id,
            name: r.name.unwrap_or_default(),
            tag: r.tag.unwrap_or_default(),
            key: r.key.unwrap_or_default(),
            inputs: r.inputs.into(),
            outputs: r.outputs.into(),
            ..Default::default()
        }
    }
}
