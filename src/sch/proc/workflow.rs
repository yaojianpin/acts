use crate::{sch::TaskState, ActTask, Context, Workflow};
use async_trait::async_trait;
use core::clone::Clone;
use serde_yaml::Value;
use std::collections::HashMap;

impl_act_state!(Workflow);
impl_act_time!(Workflow);
impl_act_id!(Workflow);

impl Workflow {
    pub(in crate::sch) fn check_pass(&self, _ctx: &Context) -> bool {
        true
    }

    pub fn is_finished(&self) -> bool {
        if self.state().is_completed() {
            return true;
        }
        let mut ret = true;
        for job in &self.jobs {
            ret &= job.state().is_completed();
        }

        ret
    }

    pub fn set_outputs(&self, inputs: HashMap<String, Value>) {
        let mut outputs = self.share_outputs.write().unwrap();
        *outputs = inputs;
    }
}

#[async_trait]
impl ActTask for Workflow {
    fn run(&self, _ctx: &Context) {}
}
