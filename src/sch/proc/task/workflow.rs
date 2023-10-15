use crate::{
    event::ActionState,
    sch::{Context, Task, TaskState},
    ActTask, Result, Workflow, WorkflowAction,
};
use async_trait::async_trait;

#[async_trait]
impl ActTask for Workflow {
    fn run(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let children = ctx.task.node.children();
        if children.len() > 0 {
            for child in &children {
                ctx.sched_task(child);
            }
        } else {
            ctx.task.set_action_state(ActionState::Completed);
        }

        Ok(children.len() > 0)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let tasks = ctx.task.children();
        let mut complete_count = 0;
        for task in tasks.iter() {
            if task.state().is_pending() && task.is_ready() {
                let ctx = task.create_context(&ctx.scher);
                task.set_state(TaskState::Running);
                ctx.scher.emitter().emit_task_event_with_extra(task, false);
                task.exec(&ctx)?;
                return Ok(false);
            }

            if task.state().is_completed() {
                complete_count += 1;
            }
        }
        if complete_count == tasks.len() {
            ctx.task.set_action_state(ActionState::Completed);
            return Ok(true);
        }

        Ok(false)
    }
}

impl Workflow {
    pub(in crate::sch) fn actions(&self, task: &Task) -> Option<Vec<&WorkflowAction>> {
        let actions = self
            .actions
            .iter()
            .filter(|iter| {
                if iter.on.len() == 0 {
                    return false;
                }

                iter.on.iter().any(|on| {
                    let mut ret = true;
                    ret &= on.state == task.action_state().to_string();

                    if let Some(nkind) = &on.nkind {
                        ret &= nkind == &task.node.kind().to_string();
                    }

                    if let Some(nid) = &on.nid {
                        ret &= nid == &task.node.id();
                    }

                    ret
                })
            })
            .collect::<Vec<_>>();

        if actions.len() > 0 {
            return Some(actions);
        }

        None
    }
}
