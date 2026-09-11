use super::super::ActModule;
use crate::{Context, Result, env::value::ActJsValue};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct SealedModule;

impl SealedModule {
    pub fn new() -> Self {
        Self
    }
}

const SEALED_SOURCE: &str = r#"
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

impl ActModule for SealedModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        let _ = JsModule::evaluate(ctx.clone(), "@acts/sealed", SEALED_SOURCE)
            .catch(ctx)
            .map_err(|err| crate::ActError::Script(err.to_string()))?;

        Ok(())
    }

    fn refresh(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        // Inject $name globals from the task's sealed chain. A local sealed
        // value overrides the parent scopes. Evaluating outside a scheduler
        // context is valid; sealed globals are simply absent.
        let mut has_sealed = false;
        let insert_globals = Context::try_with_current(|cx| -> crate::Result<()> {
            let task = cx.task();
            let mut names: Vec<String> = Vec::new();
            let mut cursor = Some(task.clone());
            while let Some(t) = cursor {
                for name in t.sealed_keys() {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                cursor = t.parent();
            }
            for name in &names {
                if let Some(data) = task.sealed(name) {
                    ctx.globals()
                        .set(format!("__sealed_{name}"), ActJsValue::new(data.into()))?;
                    has_sealed = true;
                }
            }
            Ok(())
        });
        let _ = insert_globals;

        if !has_sealed {
            return Ok(());
        }

        let _ = JsModule::evaluate(ctx.clone(), "@acts/sealed", SEALED_SOURCE)
            .catch(ctx)
            .map_err(|err| crate::ActError::Script(err.to_string()))?;

        Ok(())
    }
}
