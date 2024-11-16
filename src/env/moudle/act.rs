use crate::{ActError, ActModule, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct ActPackage;
impl ActPackage {
    pub fn new() -> Self {
        Self
    }
}

#[rquickjs::module(rename_vars = "camelCase")]
mod act {
    use crate::{
        env::value::ActValue, utils::consts, Act, ActError, Action, Block, Call, Chain, Context,
        Each, Irq, Msg, Vars,
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
    pub fn expose(key: String, value: ActValue) {
        let value: serde_json::Value = value.into();
        Context::with(|ctx| {
            let key = key.clone();
            let v = value.clone();
            let task = ctx.task();
            if let Some(parent) = task.parent() {
                parent.set_data_with(move |data| data.set(&key, v.clone()));
            } else {
                task.set_data_with(move |data| data.set(&key, v.clone()));
            }
        })
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
                &task.pid,
                &task.id,
                consts::EVT_NEXT,
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
                &task.pid,
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
            ctx.set_action(&Action::new(&task.pid, &task.id, consts::EVT_BACK, &vars))?;
            task.update_no_lock(ctx)?;
            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn skip() -> rquickjs::Result<()> {
        Context::with(|ctx| {
            let task = ctx.task();
            ctx.set_action(&Action::new(
                &task.pid,
                &task.id,
                consts::EVT_SKIP,
                &Vars::new(),
            ))?;
            task.update_no_lock(ctx)?;
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
            ctx.set_action(&Action::new(&task.pid, &task.id, consts::EVT_ERR, &vars))?;
            task.update_no_lock(ctx)?;
            Ok(())
        })
        .map_err(|err: ActError| err.into())
    }

    #[rquickjs::function]
    pub fn irq(req: ActValue) -> rquickjs::Result<()> {
        let act = Act::irq(|_| req.to::<Irq>().unwrap());
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn each(req: ActValue) -> rquickjs::Result<()> {
        let act = Act::each(move |_| req.to::<Each>().unwrap());
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn chain(req: ActValue) -> rquickjs::Result<()> {
        let act = Act::chain(|_c| req.to::<Chain>().unwrap());
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn msg(req: ActValue) -> rquickjs::Result<()> {
        let act = Act::msg(|_| req.to::<Msg>().unwrap());
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn block(req: ActValue) -> rquickjs::Result<()> {
        let act = Act::block(|_| req.to::<Block>().unwrap());
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn call(req: ActValue) -> rquickjs::Result<()> {
        let act = Act::call(|_| req.to::<Call>().unwrap());
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }

    #[rquickjs::function]
    pub fn push(act: ActValue) -> rquickjs::Result<()> {
        let act = act.to::<Act>().unwrap();
        if act.act.is_empty() {
            return Err(ActError::Action(format!(
                "'act' property is not set when pushing a new act"
            ))
            .into());
        }
        Context::with(|ctx| act.exec(ctx)).map_err(|err| err.into())
    }
}

impl ActModule for ActPackage {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        JsModule::declare_def::<js_act, _>(ctx.clone(), "@acts/act").unwrap();
        let source = r#"
        import { get, set, inputs, expose, set_output, state, complete, fail, skip, back, abort, push, irq, msg, chain, each, block, call } from '@acts/act';
        globalThis.$ = (name, value) => {
            if(value === undefined) {
                return get(name);
            }
            set(name, value);
        }
        
        globalThis.act = {
            get, set, state, inputs, expose, set_output, complete, fail, skip, back, abort, push, irq, msg, chain, each, block, call
        };
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/act", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
