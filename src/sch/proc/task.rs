use crate::{
    env::VirtualMachine,
    event::{EventAction, MessageKind},
    sch::{
        proc::Act,
        tree::{Node, NodeData},
        Context, Proc, Scheduler, TaskState,
    },
    utils, ActTask, Message, ShareLock, Vars,
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tracing::{debug, instrument};

use super::ActKind;

#[derive(Clone)]
pub struct Task {
    pub pid: String,
    pub tid: String,
    pub node: Arc<Node>,
    pub env: Arc<VirtualMachine>,

    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    // children tasks tid
    children: ShareLock<Vec<String>>,

    // previous tid
    prev: ShareLock<Option<String>>,

    pub(crate) proc: Arc<Proc>,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("kind", &self.node.kind())
            .field("pid", &self.pid)
            .field("tid", &self.tid)
            .field("nid", &self.node.id())
            .field("state", &self.state())
            .field("start_time", &self.start_time())
            .field("end_time", &self.end_time())
            .field("vars", &self.vars())
            .finish()
    }
}

impl Task {
    pub fn new(proc: &Arc<Proc>, tid: &str, node: Arc<Node>) -> Self {
        // create new env for each task
        let vm = proc.env.vm();
        if let NodeData::Job(job) = node.data() {
            // set the job env as global vars
            let vars = utils::fill_vars(&vm, &job.env);
            vm.output(&vars);
        }
        let task = Self {
            pid: proc.pid(),
            tid: tid.to_string(),
            node: node.clone(),
            state: Arc::new(RwLock::new(TaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            prev: Arc::new(RwLock::new(None)),
            children: Arc::new(RwLock::new(Vec::new())),
            env: vm,

            proc: proc.clone(),
        };

        task
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read().unwrap()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read().unwrap()
    }

    pub fn state(&self) -> TaskState {
        let state = &*self.state.read().unwrap();
        state.clone()
    }

    pub fn tid(&self) -> String {
        self.tid.clone()
    }
    pub fn nid(&self) -> String {
        self.node.id()
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
    }

    pub fn create_context(self: &Arc<Self>, scher: &Arc<Scheduler>) -> Arc<Context> {
        self.proc.create_context(scher, self)
    }

    pub fn create_message(&self, event: &EventAction) -> Message {
        let workflow = self.proc.workflow();

        let outputs = self.outputs();
        let mut vars = self.vars();
        vars.extend(outputs);

        Message {
            kind: MessageKind::Task,
            event: event.clone(),
            mid: workflow.id.clone(),
            topic: workflow.topic.clone(),
            nkind: self.node.kind().to_string(),
            nid: self.nid(),
            pid: self.pid.clone(),
            tid: self.tid.clone(),
            key: None,
            vars: vars,
        }
    }

    pub fn prev(&self) -> Option<String> {
        let ret = self.prev.read().unwrap();
        ret.clone()
    }

    pub fn parent(&self, ctx: &Context) -> Option<Arc<Task>> {
        let mut prev = self.prev();
        while let Some(tid) = prev.clone() {
            match ctx.proc.task(&tid) {
                Some(task) => {
                    if task.node.level < self.node.level {
                        return Some(task.clone());
                    }

                    prev = task.prev();
                    continue;
                }
                None => {
                    break;
                }
            }
        }

        None
    }

    pub fn children(&self) -> Vec<String> {
        let ret = self.children.read().unwrap();
        ret.clone()
    }

    pub fn acts(&self) -> Vec<Arc<Act>> {
        self.proc
            .acts()
            .iter()
            .filter(|act| act.tid == self.tid)
            .cloned()
            .collect()
    }

    pub fn vars(&self) -> Vars {
        let vars = match &self.node.data {
            NodeData::Workflow(workflow) => workflow.env.clone(),
            NodeData::Job(job) => job.env.clone(),
            NodeData::Branch(branch) => branch.env.clone(),
            NodeData::Step(step) => step.env.clone(),
        };

        utils::fill_vars(&self.env, &vars)
    }

    pub fn outputs(&self) -> Vars {
        let vars = match &self.node.data {
            NodeData::Workflow(workflow) => workflow.outputs.clone(),
            NodeData::Job(job) => job.outputs.clone(),
            _ => Vars::new(),
        };

        utils::fill_vars(&self.env, &vars)
    }

    pub fn set_prev(&self, prev: Option<String>) {
        *self.prev.write().unwrap() = prev;
    }

    pub fn set_state(&self, state: TaskState) {
        if state.is_completed() {
            self.set_end_time(utils::time::time());
        } else if state.is_running() {
            self.set_start_time(utils::time::time());
        }
        *self.state.write().unwrap() = state;
    }

    pub fn set_pure_state(&self, state: TaskState) {
        *self.state.write().unwrap() = state;
    }

    pub fn set_start_time(&self, time: i64) {
        *self.start_time.write().unwrap() = time;
    }
    pub fn set_end_time(&self, time: i64) {
        *self.end_time.write().unwrap() = time;
    }

    pub fn push_act(self: &Arc<Self>, kind: ActKind, vars: &Vars) -> Arc<Act> {
        let act = Act::new(self, kind, vars);
        self.proc.push_act(&act);

        act
    }

    pub fn push_back(&self, next: &str) {
        let mut children = self.children.write().unwrap();
        children.push(next.to_string());
    }

    #[instrument]
    pub fn exec(&self, ctx: &Context) {
        if ctx.task.state().is_none() {
            self.prepare(ctx);
        }

        if ctx.task.state().is_running() {
            self.run(ctx);
        }
        if ctx.task.state().is_next() {
            self.next(ctx);
        }

        if ctx.task.state().is_error() {
            ctx.dispatch_task(self, EventAction::Error);
        }
    }

    #[instrument]
    pub fn next(&self, ctx: &Context) {
        let node = self.node.clone();
        let children = node.children();
        if children.len() > 0 {
            for child in children {
                if self.is_cond(&child, ctx) {
                    ctx.sched_task(&child);
                }
            }
        } else {
            let next = node.next().upgrade();
            match next {
                Some(next) => {
                    self.post(ctx);
                    ctx.sched_task(&next);
                }
                None => {
                    ctx.task.post(ctx);

                    let mut parent = ctx.task.parent(ctx);
                    while let Some(task) = &parent.clone() {
                        let proc = ctx.proc.clone();
                        let ctx = &proc.create_context(&ctx.scher, task);
                        task.post(ctx);
                        if !task.state().is_completed() {
                            break;
                        }

                        let n = task.node.next().upgrade();
                        match n {
                            Some(next) => {
                                ctx.sched_task(&next);
                                break;
                            }
                            None => {
                                parent = task.parent(ctx);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn on_event(&self, event: &str, ctx: &Context) {
        let mut on_events = HashMap::new();
        if let NodeData::Step(step) = self.node.data() {
            if let Some(on) = step.on {
                on_events = on.task.clone();
            }
        }
        if let Some(event) = on_events.get(event) {
            if event.is_string() {
                let ret = ctx.run(event.as_str().unwrap());
                if !self.state().is_error() && ret.is_err() {
                    self.set_state(TaskState::Fail(ret.err().unwrap().into()));
                }
            }
        }
    }
    /// check if the condition expr is ok
    fn is_cond(&self, node: &Node, ctx: &Context) -> bool {
        let mut expr = None;
        match &node.data {
            NodeData::Branch(branch) => expr = branch.r#if.clone(),
            NodeData::Step(step) => expr = step.r#if.clone(),
            _ => {}
        }

        if let Some(expr) = expr {
            match ctx.eval(&expr) {
                Ok(ret) => {
                    debug!("check_cond {} ret={}", expr, ret);
                    return ret;
                }
                Err(_) => return false,
            }
        }

        true
    }
}

#[async_trait]
impl ActTask for Task {
    #[instrument]
    fn prepare(&self, ctx: &Context) {
        ctx.prepare();
        match &self.node.data {
            NodeData::Workflow(workflow) => {
                workflow.prepare(ctx);
            }
            NodeData::Job(job) => {
                job.prepare(ctx);
            }
            NodeData::Branch(branch) => branch.prepare(ctx),
            NodeData::Step(step) => {
                step.prepare(ctx);
            }
        }

        if self.state().is_none() {
            self.set_state(TaskState::Running);
        }

        ctx.dispatch_task(&ctx.task, EventAction::Create);
    }

    #[instrument]
    fn run(&self, ctx: &Context) {
        match &self.node.data {
            NodeData::Workflow(workflow) => workflow.run(ctx),
            NodeData::Job(job) => job.run(ctx),
            NodeData::Branch(branch) => branch.run(ctx),
            NodeData::Step(step) => step.run(ctx),
        }
    }

    #[instrument]
    fn post(&self, ctx: &Context) {
        match &self.node.data {
            NodeData::Workflow(workflow) => {
                workflow.post(ctx);
            }
            NodeData::Job(job) => job.post(ctx),
            NodeData::Branch(branch) => {
                branch.post(ctx);
            }
            NodeData::Step(step) => {
                step.post(ctx);
            }
        }

        if self.state().is_completed() {
            ctx.dispatch_task(self, EventAction::Complete);
        }
    }
}
