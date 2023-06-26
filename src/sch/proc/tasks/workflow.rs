use crate::{
    sch::{Context, TaskState},
    ActTask, Workflow,
};
use async_trait::async_trait;
use std::collections::HashMap;

#[async_trait]
impl ActTask for Workflow {
    fn run(&self, _ctx: &Context) {}

    fn post(&self, ctx: &Context) {
        // caculate all jobs state
        let mut jobs = HashMap::new();
        let mut success_count = 0;
        for job in self.jobs.iter() {
            let mut is_success = false;
            let tasks = ctx.proc.task_by_nid(&job.id);
            if tasks.len() > 0 {
                is_success = tasks.iter().all(|t| t.state().is_success());
            }
            jobs.insert(&job.id, (is_success, job));

            if is_success {
                success_count += 1;
            }
        }

        if success_count == self.jobs.len() {
            // all jobs finished, marks the workflow task as success
            ctx.task.set_state(TaskState::Success);
        } else {
            for (is_success, job) in jobs.values() {
                if !is_success {
                    let ready = job.needs.iter().all(|it| jobs.get(it).unwrap().0);
                    if ready {
                        // schedules a new job task
                        if let Some(node) = ctx.proc.node(&job.id) {
                            ctx.sched_task(&node);
                        }
                    }
                }
            }
        }
    }
}
