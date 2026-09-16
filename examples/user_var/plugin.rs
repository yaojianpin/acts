use crate::module::UserVarModule;
use acts::{ActPlugin, Result};

#[derive(Clone)]
pub struct UserVarPlugin;

#[async_trait::async_trait]
impl ActPlugin for UserVarPlugin {
    fn on_init(&self, engine: &acts::Engine) -> Result<()> {
        engine
            .executor(&acts::Principal::unrestricted())
            .ext()
            .register_var(&UserVarModule)
            .unwrap();
        Ok(())
    }
}
