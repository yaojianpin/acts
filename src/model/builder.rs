use super::{act::ActCatch, workflow::WorkflowActionOn};
use crate::{Act, ActAlias, ActFor, ActValue, Branch, Step, Workflow, WorkflowAction};

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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
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

    pub fn with_action(mut self, build: fn(WorkflowAction) -> WorkflowAction) -> Self {
        let action = WorkflowAction::default();
        self.actions.push(build(action));
        self
    }
    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_act(mut self, build: fn(Act) -> Act) -> Self {
        let act = Act::default();
        let act = build(act);
        self.acts.push(act);
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

    pub fn with_env(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_input(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    pub fn with_else(mut self, default: bool) -> Self {
        self.r#else = default;
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

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.steps.push(build(step));
        self
    }

    pub fn with_need(mut self, need: &str) -> Self {
        self.needs.push(need.to_string());
        self
    }
}

impl Act {
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

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_need(mut self, need: &str) -> Self {
        self.needs.push(need.to_string());
        self
    }

    pub fn with_for(mut self, build: fn(ActFor) -> ActFor) -> Self {
        let f = ActFor::default();
        self.r#for = Some(build(f));
        self
    }

    pub fn with_catch(mut self, build: fn(ActCatch) -> ActCatch) -> Self {
        let catch = ActCatch::default();
        self.catches.push(build(catch));
        self
    }

    pub fn with_use(mut self, mid: &str) -> Self {
        self.r#use = Some(mid.to_string());
        self
    }
}

impl ActFor {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_in(mut self, scr: &str) -> Self {
        self.r#in = scr.to_string();
        self
    }

    pub fn with_by(mut self, by: &str) -> Self {
        self.by = by.to_string();
        self
    }

    pub fn with_alias(mut self, build: fn(ActAlias) -> ActAlias) -> Self {
        let alias = ActAlias::default();
        self.alias = build(alias);
        self
    }
}

impl ActAlias {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_init(mut self, v: &str) -> Self {
        self.init = Some(v.to_string());
        self
    }

    pub fn with_each(mut self, v: &str) -> Self {
        self.each = Some(v.to_string());
        self
    }
}

impl WorkflowAction {
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_on(mut self, build: fn(WorkflowActionOn) -> WorkflowActionOn) -> Self {
        let on = WorkflowActionOn::default();
        self.on.push(build(on));
        self
    }
}

impl WorkflowActionOn {
    pub fn with_state(mut self, state: &str) -> Self {
        self.state = state.to_string();
        self
    }

    pub fn with_nkind(mut self, nkind: &str) -> Self {
        self.nkind = Some(nkind.to_string());
        self
    }

    pub fn with_nid(mut self, nid: &str) -> Self {
        self.nid = Some(nid.to_string());
        self
    }
}

impl ActCatch {
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_input(mut self, name: &str, value: ActValue) -> Self {
        self.inputs.insert(name.to_string(), value);
        self
    }

    pub fn with_output(mut self, name: &str, value: ActValue) -> Self {
        self.outputs.insert(name.to_string(), value);
        self
    }

    pub fn with_err(mut self, err: &str) -> Self {
        self.err = Some(err.to_string());
        self
    }
}
