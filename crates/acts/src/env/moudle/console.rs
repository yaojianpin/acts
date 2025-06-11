use crate::{Result, env::ActModule};
use rquickjs::{JsLifetime, class::Trace};

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

    fn log(&self, message: String) {
        println!("[log] {message}");
    }

    fn info(&self, message: String) {
        println!("[info] {}", message);
    }

    fn warn(&self, message: String) {
        println!("[warn] {}", message);
    }

    fn error(&self, message: String) {
        println!("[error] {}", message);
    }
}

impl ActModule for ConsoleModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        ctx.globals().set("console", self.clone())?;

        Ok(())
    }
}
