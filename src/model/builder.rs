use serde_yaml::Value;

use crate::{
    step::{Action, Subject},
    Branch, Job, Step, Workflow,
};

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

    pub fn with_ver(mut self, ver: &str) -> Self {
        self.ver = ver.to_string();
        self
    }

    pub fn with_env(mut self, name: &str, value: Value) -> Self {
        self.env.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: Value) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_on(mut self, name: &str, value: Value) -> Self {
        self.on.insert(name.to_string(), value.into());
        self
    }

    pub fn with_biz_id(mut self, biz_id: &str) -> Self {
        self.set_biz_id(biz_id);
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

    pub fn with_env(mut self, name: &str, value: Value) -> Self {
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

    pub fn with_output(mut self, name: &str, value: Value) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_on(mut self, name: &str, value: Value) -> Self {
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

    pub fn with_accept(mut self, accept: &str) -> Self {
        self.accept = Some(accept.into());
        self
    }

    pub fn with_action(mut self, act: Action) -> Self {
        self.action = Some(act);
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

    pub fn with_on(mut self, name: &str, value: Value) -> Self {
        self.on.insert(name.to_string(), value.into());
        self
    }

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
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

    pub fn with_accept(mut self, accept: &str) -> Self {
        self.accept = Some(accept.into());
        self
    }

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

    pub fn with_users(mut self, users: &str) -> Self {
        self.users = users.to_string();
        self
    }

    pub fn with_on(mut self, name: &str, value: Value) -> Self {
        self.on.insert(name.to_string(), value.into());
        self
    }
}

// impl Action {
//     pub fn new() -> Self {
//         Default::default()
//     }
//     pub fn with_name(mut self, name: &str) -> Self {
//         self.name = name.to_string();
//         self
//     }

//     pub fn with_param(mut self, name: &str, value: Value) -> Self {
//         self.with.insert(name.to_string(), value.into());
//         self
//     }
// }
