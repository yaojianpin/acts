use crate::{ActValue, Branch, Job, Step, Subject, Workflow};

use super::step::OnCallback;

impl Workflow {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_env(mut self, name: &str, value: ActValue) -> Self {
        self.env.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_on(mut self, name: &str, value: ActValue) -> Self {
        self.on.insert(name.to_string(), value.into());
        self
    }

    pub fn with_job(mut self, build: fn(Job) -> Job) -> Self {
        let job = Job::default();
        let job = build(job);
        self.jobs.push(job);
        self
    }
}

impl Job {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_env(mut self, name: &str, value: ActValue) -> Self {
        self.env.insert(name.to_string(), value);
        self
    }

    pub fn with_accept(mut self, accept: &str) -> Self {
        self.accept = Some(accept.into());
        self
    }

    pub fn with_need(mut self, need: &str) -> Self {
        self.needs.push(need.to_string());
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_on(mut self, name: &str, value: ActValue) -> Self {
        self.on.insert(name.to_string(), value.into());
        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        let step = build(step);
        self.steps.push(step);
        self
    }
}

impl Step {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_action(mut self, act: &str) -> Self {
        self.action = Some(act.to_string());
        self
    }

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    pub fn with_if(mut self, r#if: &str) -> Self {
        self.r#if = Some(r#if.to_string());
        self
    }

    pub fn with_run(mut self, run: &str) -> Self {
        self.run = Some(run.to_string());
        self
    }

    pub fn with_subject(mut self, build: fn(Subject) -> Subject) -> Self {
        let sub = Subject::default();
        self.subject = Some(build(sub));
        self
    }

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
        self
    }

    pub fn with_on(mut self, build: fn(OnCallback) -> OnCallback) -> Self {
        let on = OnCallback::default();
        self.on = Some(build(on));
        self
    }
}

impl Branch {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    // pub fn with_accept(mut self, accept: &str) -> Self {
    //     self.accept = Some(accept.into());
    //     self
    // }

    pub fn with_if(mut self, r#if: &str) -> Self {
        self.r#if = Some(r#if.to_string());
        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
        self
    }
}

impl Subject {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_matcher(mut self, m: &str) -> Self {
        self.matcher = m.to_string();
        self
    }

    pub fn with_cands(mut self, cands: &str) -> Self {
        self.cands = cands.to_string();
        self
    }
}

impl OnCallback {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_task(mut self, name: &str, value: ActValue) -> Self {
        self.task.insert(name.to_string(), value);
        self
    }

    pub fn with_act(mut self, name: &str, value: ActValue) -> Self {
        self.act.insert(name.to_string(), value);
        self
    }
}
