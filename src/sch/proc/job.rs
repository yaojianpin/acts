use crate::{
    model::Job,
    sch::{Context, TaskState},
    ActTask, Workflow,
};
use async_trait::async_trait;
use core::clone::Clone;
use std::{ops::Deref, sync::Arc};

impl_act_state!(Job);
impl_act_time!(Job);
impl_act_id!(Job);

impl Job {
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

    pub fn set_workflow(&self, workflow: Box<Workflow>) {
        *self.workflow.write().unwrap() = workflow;
    }

    pub fn workflow(&self) -> Box<Workflow> {
        let workflow = self.workflow.read().unwrap();
        workflow.clone()
    }

    pub fn needs(&self) -> Vec<Job> {
        let workflow = self.workflow();
        let mut ret = Vec::new();
        workflow.jobs.iter().for_each(|job| {
            if self.needs.contains(&job.id) {
                ret.push(job.clone());
            }
        });

        ret
    }
}

#[async_trait]
impl ActTask for Job {
    fn run(&self, _ctx: &Context) {}
}

impl From<Arc<Job>> for Job {
    fn from(item: Arc<Job>) -> Self {
        item.deref().clone()
    }
}
