use super::super::ActModule;
use crate::{ActError, Context, Result, Vars, env::value::ActValue};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct ActJsModule;
impl ActJsModule {
    pub fn new() -> Self {
        Self
    }

    pub fn vars(&self) -> Option<Vars> {
        if let Ok(ctx) = Context::current() {
            return Some(ctx.task().vars());
        }
        None
    }
}

#[allow(clippy::module_inception)]
#[rquickjs::module(rename_vars = "camelCase")]
mod act {
    use crate::{Context, Vars, env::value::ActValue};

    #[rquickjs::function]
    pub fn get_act_value(name: String) -> Option<ActValue> {
        Context::with(|ctx| {
            if let Some(v) = ctx.task().find(&name) {
                let v = ActValue::new(v);
                return Some(v);
            }
            None
        })
    }

    #[rquickjs::function]
    pub fn set_act_value(name: String, value: ActValue) {
        Context::with(|ctx| {
            let vars = Vars::new().with(&name, value.inner());
            ctx.task().update_data(&vars);
        })
    }

    #[rquickjs::function]
    pub fn set_process_var(name: String, value: ActValue) {
        Context::with(|ctx| {
            let vars = Vars::new().with(&name, value.inner());
            ctx.proc.set_data(&vars);
        })
    }

    #[rquickjs::function]
    pub fn get_act_inputs() -> ActValue {
        Context::with(|ctx| ctx.task().inputs().into())
    }

    #[rquickjs::function]
    pub fn get_act_data() -> ActValue {
        Context::with(|ctx| ctx.task().data().into())
    }
}

impl ActModule for ActJsModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        JsModule::declare_def::<js_act, _>(ctx.clone(), "@acts/act").unwrap();

        if let Some(vars) = self.vars() {
            for (key, value) in &vars {
                ctx.globals().set(&key, ActValue::new(value))?;
            }
        }

        let source = r#"
        import { get_act_value, set_act_value, set_process_var, get_act_inputs, get_act_data } from '@acts/act';

        globalThis.$get = get_act_value;
        globalThis.$set = set_act_value;
        globalThis.$inputs = get_act_inputs;
        globalThis.$data = get_act_data;
        globalThis.$set_process_var = set_process_var;
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/act", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
