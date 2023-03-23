use crate::{adapter::SomeRule, debug, ActError, ActResult, TaskState};

#[derive(Debug, Default)]
pub struct Rate;

impl SomeRule for Rate {
    fn some(&self, _step: &crate::Step, ctx: &crate::Context) -> ActResult<bool> {
        let acts = ctx.proc.children(&ctx.task);
        if acts.len() == 0 {
            return Ok(false);
        }

        #[allow(unused_assignments)]
        let mut rate = 0_f32;
        match ctx.var("rate") {
            Some(v) => rate = v.as_f64().unwrap() as f32,
            None => return Err(ActError::RuntimeError("cannot find env.rate".to_string())),
        }

        let success_count = acts
            .iter()
            .filter(|act| act.state() == TaskState::Success)
            .count() as f32;

        let current = success_count / acts.len() as f32;
        debug!("some::rate:{}/{}", current, rate);
        Ok(current >= rate)
    }
}
