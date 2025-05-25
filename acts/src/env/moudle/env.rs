use crate::{ActError, ActModule, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct Env;
impl Env {
    pub fn new() -> Self {
        Self
    }
}

#[allow(clippy::module_inception)]
#[rquickjs::module(rename_vars = "camelCase")]
mod env {
    use crate::{Context, env::value::ActValue};

    #[rquickjs::function]
    pub fn get(name: String) -> Option<ActValue> {
        Context::with(|ctx| ctx.get_env(&name).map(ActValue::new))
    }

    #[rquickjs::function]
    pub fn set(name: String, value: ActValue) {
        Context::with(|ctx| {
            ctx.set_env(&name, value.inner());
        })
    }
}

impl ActModule for Env {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
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
