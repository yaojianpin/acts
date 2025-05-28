use acts::{ActUserVar, Vars};

#[derive(Clone)]
pub struct UserVarModule;

impl ActUserVar for UserVarModule {
    fn name(&self) -> String {
        "test".to_string()
    }

    fn default_data(&self) -> Option<acts::Vars> {
        Some(Vars::new().with("var1", "a").with("var2", 10))
    }
}
