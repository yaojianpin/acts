use crate::{
    debug,
    sch::{ActId, ActState, TaskState},
    Act, ActTask, Branch, Context, Job, Step, Workflow,
};
use async_trait::async_trait;

#[derive(Clone)]
pub enum Task {
    Workflow(String, Workflow),
    Job(String, Job),
    Branch(String, Branch),
    Step(String, Step),
    Act(String, Act),
}

impl Task {
    pub fn check_pass(&self, ctx: &Context) -> bool {
        let pass = match self {
            Task::Workflow(_, workflow) => workflow.check_pass(ctx),
            Task::Job(_, job) => job.check_pass(ctx),
            Task::Branch(_, branch) => branch.check_pass(ctx),
            Task::Step(_, step) => step.check_pass(ctx),
            Task::Act(_, act) => act.check_pass(ctx),
        };
        debug!("check_pass: {}", pass);

        pass
    }

    pub fn start_time(&self) -> i64 {
        match self {
            Task::Workflow(_, workflow) => workflow.start_time(),
            Task::Job(_, job) => job.start_time(),
            Task::Branch(_, branch) => branch.start_time(),
            Task::Step(_, step) => step.start_time(),
            Task::Act(_, act) => act.start_time(),
        }
    }

    pub fn set_start_time(&self, time: i64) {
        match self {
            Task::Workflow(_, workflow) => workflow.set_start_time(time),
            Task::Job(_, job) => job.set_start_time(time),
            Task::Branch(_, branch) => branch.set_start_time(time),
            Task::Step(_, step) => step.set_start_time(time),
            Task::Act(_, act) => act.set_start_time(time),
        }
    }

    pub fn set_end_time(&self, time: i64) {
        match self {
            Task::Workflow(_, workflow) => workflow.set_end_time(time),
            Task::Job(_, job) => job.set_end_time(time),
            Task::Branch(_, branch) => branch.set_end_time(time),
            Task::Step(_, step) => step.set_end_time(time),
            Task::Act(_, act) => act.set_end_time(time),
        }
    }

    pub fn set_user(&self, user: &str) {
        match self {
            Task::Act(_, act) => act.set_user(user),
            _ => {}
        }
    }

    pub fn end_time(&self) -> i64 {
        match self {
            Task::Workflow(_, workflow) => workflow.end_time(),
            Task::Job(_, job) => job.end_time(),
            Task::Branch(_, branch) => branch.end_time(),
            Task::Step(_, step) => step.end_time(),
            Task::Act(_, act) => act.end_time(),
        }
    }

    pub fn user(&self) -> String {
        match self {
            Task::Workflow(_, _) => "".to_string(),
            Task::Job(_, _) => "".to_string(),
            Task::Branch(_, _) => "".to_string(),
            Task::Step(_, _) => "".to_string(),
            Task::Act(_, act) => {
                let user = act.user();
                match user {
                    Some(u) => u.clone(),
                    None => "".to_string(),
                }
            }
        }
    }

    pub fn needs(&self) -> Vec<String> {
        match self {
            Task::Workflow(_, _workflow) => vec![],
            Task::Job(_, job) => job.needs.clone(),
            Task::Branch(_, _branch) => vec![],
            Task::Step(_, _step) => vec![],
            Task::Act(_, _act) => vec![],
        }
    }

    pub fn as_job(&self) -> Option<&Job> {
        match self {
            Task::Job(_, job) => Some(job),
            _ => None,
        }
    }

    pub fn next(&self, ctx: &Context) {
        let next_ = |ctx: &Context| {
            if let Some(node) = ctx.proc.tree.node(&self.tid()) {
                ctx.proc.next(&node, ctx)
            }
        };
        match self {
            Task::Workflow(_, _) => {
                next_(ctx);
            }
            Task::Job(_, _) | Task::Branch(_, _) => {
                next_(ctx);
            }
            Task::Step(_, _) => {
                next_(ctx);
            }
            Task::Act(_, act) => {
                if let Some(step) = act.parent(ctx) {
                    if step.state().is_completed() {
                        return;
                    }
                    let ret = step.check_pass(ctx);
                    if ret {
                        step.set_state(&TaskState::Success);
                        next_(ctx);
                    }
                }
            }
        }
    }
}

impl ActId for Task {
    fn tid(&self) -> String {
        match self {
            Task::Workflow(tid, _) => tid.clone(),
            Task::Job(tid, _) => tid.clone(),
            Task::Branch(tid, _) => tid.clone(),
            Task::Step(tid, _) => tid.clone(),
            Task::Act(tid, _) => tid.clone(),
        }
    }
}
impl ActState for Task {
    fn state(&self) -> TaskState {
        match self {
            Task::Workflow(_, workflow) => workflow.state(),
            Task::Job(_, job) => job.state(),
            Task::Branch(_, branch) => branch.state(),
            Task::Step(_, step) => step.state(),
            Task::Act(_, act) => act.state(),
        }
    }

    fn set_state(&self, state: &TaskState) {
        match self {
            Task::Workflow(_, workflow) => workflow.set_state(state),
            Task::Job(_, job) => job.set_state(state),
            Task::Branch(_, branch) => branch.set_state(state),
            Task::Step(_, step) => step.set_state(state),
            Task::Act(_, act) => act.set_state(state),
        }
    }
}

#[async_trait]
impl ActTask for Task {
    fn prepare(&self, ctx: &Context) {
        debug!("prepare:{}", self.tid());
        if self.state().is_completed() {
            return;
        }
        ctx.prepare(self);
        match self {
            Task::Workflow(_, workflow) => {
                workflow.prepare(ctx);
            }
            Task::Job(_, job) => {
                job.prepare(ctx);
            }
            Task::Branch(_, branch) => branch.prepare(ctx),
            Task::Step(_, step) => {
                step.prepare(ctx);
            }
            Task::Act(_, act) => act.prepare(ctx),
        }
    }

    fn run(&self, ctx: &Context) {
        if self.state().is_completed() {
            return;
        }
        debug!("run:{} {:?}", self.tid(), self.state());
        match self {
            Task::Workflow(_, workflow) => workflow.run(ctx),
            Task::Job(_, job) => job.run(ctx),
            Task::Branch(_, branch) => branch.run(ctx),
            Task::Step(_, step) => step.run(ctx),
            Task::Act(_, act) => {
                act.run(ctx);
            }
        }
    }

    fn post(&self, ctx: &Context) {
        match self {
            Task::Workflow(_, workflow) => workflow.post(ctx),
            Task::Job(_, job) => job.post(ctx),
            Task::Branch(_, branch) => branch.post(ctx),
            Task::Step(_, step) => step.post(ctx),
            Task::Act(_, act) => act.post(ctx),
        }
        ctx.post(self);
        debug!("post:{} {:?}", self.tid(), self.state());
    }
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Workflow(id, workflow) => write!(f, "({} {})", id, workflow.name),
            Task::Job(id, job) => write!(f, "({} {})", id, job.name),
            Task::Branch(id, branch) => write!(f, "({} {})", id, branch.name),
            Task::Step(id, step) => write!(f, "({} {})", id, step.name),
            Task::Act(id, act) => write!(f, "({} {})", id, act.owner),
        }
    }
}
