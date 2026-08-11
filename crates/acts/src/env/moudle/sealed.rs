use super::super::ActModule;
use crate::{Context, Result, env::value::ActJsValue};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct SealedModule;

impl SealedModule {
    pub fn new() -> Self {
        Self
    }
}

impl ActModule for SealedModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        // Only inject $name globals when inside a task context
        if let Ok(cx) = Context::current() {
            let task = cx.task();
            if task.has_sealed() {
                let keys = task.sealed_keys();
                for name in &keys {
                    if let Some(data) = task.sealed(name) {
                        ctx.globals()
                            .set(format!("__sealed_{name}"), ActJsValue::new(data.into()))?;
                    }
                }
            }
        }

        let source = r#"
        (function() {
            function deepFreeze(obj) {
                if (typeof obj !== 'object' || obj === null) return obj;
                Object.keys(obj).forEach(function(k) {
                    deepFreeze(obj[k]);
                });
                return Object.freeze(obj);
            }
            var keys = Object.keys(globalThis).filter(function(k) {
                return k.startsWith('__sealed_');
            });
            for (var i = 0; i < keys.length; i++) {
                var name = keys[i];
                var publicName = '$' + name.slice(9);
                globalThis[publicName] = deepFreeze(globalThis[name]);
                delete globalThis[name];
            }
        })();
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/sealed", source)
            .catch(ctx)
            .map_err(|err| crate::ActError::Script(err.to_string()))?;

        Ok(())
    }
}
