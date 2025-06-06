use crate::module::UserVarModule;
use acts::{ActPlugin, Result};

#[derive(Clone)]
pub struct UserVarPlugin;

#[async_trait::async_trait]
impl ActPlugin for UserVarPlugin {
    async fn on_init(&self, engine: &acts::Engine) -> Result<()> {
        engine.extender().register_var(&UserVarModule);
        Ok(())
    }
}
