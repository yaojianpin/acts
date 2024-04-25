use crate::{ActError, ActModule, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct Env;
impl Env {
    pub fn new() -> Self {
        Self
    }
}

#[rquickjs::module(rename_vars = "camelCase")]
mod env {
    use crate::{env::value::ActValue, Context, Vars};

    #[rquickjs::function]
    pub fn get(name: String) -> Option<ActValue> {
        Context::with(|ctx| {
            // find the env from env local firstly
            if let Some(v) = ctx.proc.with_env_local(|vars| vars.get(&name)) {
                let v = ActValue::new(v);
                return Some(v);
            }

            // then get the value from global env
            if let Some(v) = ctx.env.get(&name) {
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

            // in context, the global env is not writable
            // just set the value to local env of the proc
            ctx.proc.with_env_local_mut(|data| {
                for (k, v) in vars.iter() {
                    data.set(k, v.clone());
                }
            });
        })
    }
}

impl ActModule for Env {
    fn init<'js>(&self, ctx: &rquickjs::Ctx<'js>) -> Result<()> {
        JsModule::declare_def::<js_env, _>(ctx.clone(), "@acts/env").unwrap();

        let source = r#"
        import { get, set } from '@acts/env';
        globalThis.$env = (name, value) => {
            if(value === undefined) {
                return get(name);
            }
            set(name, value);
        }"#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/env", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
