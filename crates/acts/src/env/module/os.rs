use super::super::ActModule;
use crate::Result;

pub struct Os;

impl Os {
    pub fn new() -> Self {
        Self
    }
}

impl ActModule for Os {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        ctx.globals().set("os", std::env::consts::OS)?;
        Ok(())
    }
}
