use crate::{Result, env::ActModule};
use rquickjs::{
    Ctx, Function, JsLifetime, String as JsString, Value, class::Trace, function::Rest,
};

#[derive(Trace, Clone, JsLifetime)]
#[rquickjs::class]
pub struct ConsoleModule {}

impl Default for ConsoleModule {
    fn default() -> Self {
        Self::new()
    }
}

#[rquickjs::methods]
impl ConsoleModule {
    pub fn new() -> Self {
        ConsoleModule {}
    }

    fn log<'js>(&self, ctx: Ctx<'js>, rest: Rest<Value<'js>>) {
        println!("[log] {}", format_rest(ctx, rest.0));
    }

    fn info<'js>(&self, ctx: Ctx<'js>, rest: Rest<Value<'js>>) {
        println!("[info] {}", format_rest(ctx, rest.0));
    }

    fn warn<'js>(&self, ctx: Ctx<'js>, rest: Rest<Value<'js>>) {
        println!("[warn] {}", format_rest(ctx, rest.0));
    }

    fn error<'js>(&self, ctx: Ctx<'js>, rest: Rest<Value<'js>>) {
        println!("[error] {}", format_rest(ctx, rest.0));
    }
}

/// Coerce each JS value to string via JS `String(v)`, then join with spaces.
fn format_rest<'js>(ctx: Ctx<'js>, args: Vec<Value<'js>>) -> String {
    let string_fn: Function<'js> = ctx
        .globals()
        .get("String")
        .unwrap_or_else(|_| panic!("String constructor not found"));
    args.into_iter()
        .map(|v| {
            string_fn
                .call::<_, JsString>((v,))
                .and_then(|s| s.to_string())
                .unwrap_or_else(|_| "?".to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl ActModule for ConsoleModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        ctx.globals().set("console", self.clone())?;

        Ok(())
    }
}
