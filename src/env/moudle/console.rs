use crate::{env::ActModule, Result};
use rquickjs::{class::Trace, JsLifetime};

#[derive(Trace, Clone, JsLifetime)]
#[rquickjs::class]
pub struct Console {}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

#[rquickjs::methods]
impl Console {
    pub fn new() -> Self {
        Console {}
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

impl ActModule for Console {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        ctx.globals().set("console", self.clone())?;

        Ok(())
    }
}
