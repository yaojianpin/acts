use super::super::ActModule;
use crate::{ActError, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct ProcEnv;
impl ProcEnv {
    pub fn new() -> Self {
        Self
    }
}

#[allow(clippy::module_inception)]
#[rquickjs::module(rename_vars = "camelCase")]
mod env {
    use crate::{Context, Result, env::value::ActJsValue};
    #[rquickjs::function]
    pub fn get_env(name: String) -> Option<ActJsValue> {
        // Private keys (the process owner credential) are engine-internal:
        // exposing one would let a model read — and, through `set_env`, forge
        // — its own snapshot scope authority.
        if crate::utils::consts::is_private_key(&name) {
            return None;
        }
        Context::with(|ctx| ctx.get_env(&name).map(ActJsValue::new))
    }

    #[rquickjs::function]
    pub fn set_env(name: String, value: ActJsValue) -> Result<()> {
        if crate::utils::consts::is_private_key(&name) {
            return Ok(());
        }
        Context::with(|ctx| {
            ctx.set_env(&name, value.inner());
        });

        Ok(())
    }
}

impl ActModule for ProcEnv {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        JsModule::declare_def::<js_env, _>(ctx.clone(), "@acts/env").unwrap();

        let source = r#"
        import { get_env, set_env } from '@acts/env';
        const handler = {
            get(target, prop) {
                return get_env(prop);
            },
            set(target, prop, value) {
                set_env(prop, value);
                return true;
            },
        }
        globalThis.$env = new Proxy({}, handler);
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/env", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
