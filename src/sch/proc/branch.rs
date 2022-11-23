use crate::{
    model::Branch,
    sch::{Context, TaskState},
    ActError, ActTask,
};
use async_trait::async_trait;
use core::clone::Clone;

impl_act_state!(Branch);
impl_act_time!(Branch);
impl_act_id!(Branch);

impl Branch {
    pub(in crate::sch) fn check_pass(&self, ctx: &Context) -> bool {
        match &self.accept {
            Some(m) => {
                if m.is_sequence() {
                    let seq = m.as_sequence().unwrap();
                    return seq.iter().all(|evt| {
                        let key = evt.as_str().unwrap();
                        ctx.user_data().action == key
                    });
                }

                true
            }
            None => true,
        }
    }
}

#[async_trait]
impl ActTask for Branch {
    fn run(&self, ctx: &Context) {
        if let Some(expr) = &self.r#if {
            match ctx.eval(expr) {
                Ok(cond) => {
                    if cond {
                        self.set_state(&TaskState::Success);
                    } else {
                        self.set_state(&TaskState::Skip);
                    }
                }
                Err(err) => self.set_state(&TaskState::Fail(err.into())),
            }
        } else {
            self.set_state(&TaskState::Fail(ActError::BranchIfError.into()));
        }
    }
}
