use crate::{env::ActModule, Result};
use rquickjs::class::Trace;

#[derive(Trace, Clone)]
#[rquickjs::class]
pub struct Console {}

#[rquickjs::methods]
impl Console {
    pub fn new() -> Self {
        Console {}
    }

    fn log(&self, message: String) {
        println!("[log] {message}");
    }

    fn info(&self, message: String) {
        println!("{}", format!("[info]{}", message));
    }

    fn wran(&self, message: String) {
        println!("{}", format!("[wran]{}", message));
    }

    fn error(&self, message: String) {
        println!("{}", format!("[error]{}", message));
    }
}

impl ActModule for Console {
    fn init<'a>(&self, ctx: &rquickjs::Ctx<'a>) -> Result<()> {
        ctx.globals().set("console", self.clone())?;

        Ok(())
    }
}
