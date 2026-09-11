pub mod secrets;

use super::super::ActModule;
use crate::{
    Context, Result, Vars,
    env::{Enviroment, value::ActJsValue},
};

pub struct UserVars {
    env: Enviroment,
}
impl UserVars {
    pub fn new(env: &Enviroment) -> Self {
        Self { env: env.clone() }
    }

    pub fn get_data(&self, key: &str) -> Option<Vars> {
        Context::try_with_current(|ctx| ctx.task().find::<Vars>(key))
            .ok()
            .flatten()
    }
}

impl ActModule for UserVars {
    fn init(&self, _ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        Ok(())
    }

    fn refresh(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        let envs = self.env.user_vars.read();
        for env in envs.iter() {
            let name = env.name();
            let mut data = Vars::new();
            if let Some(vars) = env.default_data() {
                data = vars;
            }
            if let Some(vars) = self.get_data(&name) {
                for (k, v) in vars.iter() {
                    data.set(k, v);
                }
            }
            ctx.globals()
                .set(env.name(), ActJsValue::new(data.into()))?;
        }
        Ok(())
    }
}
