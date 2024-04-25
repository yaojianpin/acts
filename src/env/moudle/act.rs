use crate::{ActError, ActModule, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct Act;
impl Act {
    pub fn new() -> Self {
        Self
    }
}

#[rquickjs::module(rename_vars = "camelCase")]
mod act {
    use crate::{
        env::value::ActValue, utils::consts, Act, ActError, Action, Block, Call, Chain, Context,
        Each, Msg, Req, Vars,
    };

    #[rquickjs::function]
    pub fn get(name: String) -> Option<ActValue> {
        Context::with(|ctx| {
            if let Some(v) = ctx.task().find(&name) {
                let v = ActValue::new(v);
                return Some(v);
            }
            None
        })
    }

    #[rquickjs::function]
    pub fn set(name: String, value: ActValue) {
        Context::with(|ctx| {
            let vars = Vars::new().with(&name, value.inner());
            ctx.task().update_data(&vars);
        })
    }

    #[rquickjs::function]
    pub fn inputs() -> ActValue {
        Context::with(|ctx| ctx.task().inputs().into())
    }

    #[rquickjs::function]
    pub fn set_output(key: String, value: ActValue) {
        let value: serde_json::Value = value.into();
        Context::with(|ctx| {
            let key = key.clone();
            let v = value.clone();
            ctx.task().set_data_with(|data| {
                let outputs = data.get::<Vars>(consts::ACT_OUTPUTS).unwrap_or_default();
                data.set(consts::ACT_OUTPUTS, outputs.with(&key, &v));
            })
        })
    }

    #[rquickjs::function]
    pub fn state() -> rquickjs::Result<ActValue> {
        Context::with(|ctx| {
            let task = ctx.task();
            Ok(task.state().to_string().into())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn complete() -> rquickjs::Result<()> {
        Context::with(|ctx| {
            let task = ctx.task();
            ctx.set_action(&Action::new(
                &task.proc_id,
                &task.id,
                consts::EVT_COMPLETE,
                &Vars::new(),
            ))?;
            task.update_no_lock(ctx)?;
            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn abort() -> rquickjs::Result<()> {
        Context::with(|ctx| {
            let task = ctx.task();
            ctx.set_action(&Action::new(
                &task.proc_id,
                &task.id,
                consts::EVT_ABORT,
                &Vars::new(),
            ))?;
            task.update_no_lock(ctx)?;

            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn back(nid: String) -> rquickjs::Result<()> {
        let vars = Vars::new().with(consts::ACT_TO, nid);
        Context::with(|ctx| {
            let task = ctx.task();
            ctx.set_action(&Action::new(
                &task.proc_id,
                &task.id,
                consts::EVT_BACK,
                &vars,
            ))?;
            task.update_no_lock(&ctx)?;
            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn skip() -> rquickjs::Result<()> {
        Context::with(|ctx| {
            let task = ctx.task();
            ctx.set_action(&Action::new(
                &task.proc_id,
                &task.id,
                consts::EVT_SKIP,
                &Vars::new(),
            ))?;
            task.update_no_lock(&ctx)?;
            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn fail(ecode: String, message: String) -> rquickjs::Result<()> {
        let vars = Vars::new()
            .with(consts::ACT_ERR_CODE, ecode)
            .with(consts::ACT_ERR_MESSAGE, message);
        Context::with(|ctx| {
            let task = ctx.task();
            ctx.set_action(&Action::new(
                &task.proc_id,
                &task.id,
                consts::EVT_ERR,
                &vars,
            ))?;
            task.update_no_lock(&ctx)?;
            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn req(req: ActValue) -> rquickjs::Result<()> {
        let req = req.to::<Req>().unwrap();
        let act = Act::Req(req);
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn each(req: ActValue) -> rquickjs::Result<()> {
        let each = req.to::<Each>().unwrap();
        let act = Act::Each(each);
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn chain(req: ActValue) -> rquickjs::Result<()> {
        let chain = req.to::<Chain>().unwrap();
        let act = Act::Chain(chain);
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn msg(req: ActValue) -> rquickjs::Result<()> {
        let msg = req.to::<Msg>().unwrap();
        let act = Act::Msg(msg);
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn block(req: ActValue) -> rquickjs::Result<()> {
        let block = req.to::<Block>().unwrap();
        let act = Act::Block(block);
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn call(req: ActValue) -> rquickjs::Result<()> {
        let call = req.to::<Call>().unwrap();
        let act = Act::Call(call);
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn push(act: ActValue) -> rquickjs::Result<()> {
        let act = act.to::<Act>().unwrap();
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }
}

impl ActModule for Act {
    fn init<'js>(&self, ctx: &rquickjs::Ctx<'js>) -> Result<()> {
        JsModule::declare_def::<js_act, _>(ctx.clone(), "@acts/act").unwrap();
        let source = r#"
        import { get, set, inputs, set_output, state, complete, fail, skip, back, abort, push, req, msg, chain, each, block, call } from '@acts/act';
        globalThis.$ = (name, value) => {
            if(value === undefined) {
                return get(name);
            }
            set(name, value);
        }
        
        globalThis.act = {
            get, set, state, inputs, set_output, complete, fail, skip, back, abort, push, req, msg, chain, each, block, call
        };
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/act", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
