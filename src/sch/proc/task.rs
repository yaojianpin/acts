use crate::{
    debug,
    sch::{
        tree::{Node, NodeData},
        ActId, ActState, EventAction, TaskState,
    },
    utils, ActTask, Context, ShareLock, State,
};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct Task {
    pub pid: String,
    pub tid: String,
    pub node: Arc<Node>,

    uid: ShareLock<Option<String>>,
    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    // children tasks tid
    children: ShareLock<Vec<String>>,

    // previous tid
    prev: ShareLock<Option<String>>,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("pid", &self.pid)
            .field("tid", &self.tid)
            .field("nid", &self.node.id())
            .field("kind", &self.node.kind())
            .field("state", &self.state.read().unwrap())
            .field("start_time", &self.start_time.read().unwrap())
            .field("end_time", &self.end_time.read().unwrap())
            .field("uid", &self.uid.read().unwrap())
            .finish()
    }
}

impl Task {
    pub fn new(pid: &str, tid: &str, node: Arc<Node>) -> Self {
        let task = Self {
            pid: pid.to_string(),
            tid: tid.to_string(),
            node: node.clone(),
            state: Arc::new(RwLock::new(TaskState::None)),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            uid: Arc::new(RwLock::new(node.data().owner())),

            prev: Arc::new(RwLock::new(None)),
            children: Arc::new(RwLock::new(Vec::new())),
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

    pub fn uid(&self) -> Option<String> {
        self.uid.read().unwrap().clone()
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
                        return Some(task);
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

    pub(crate) fn set_prev(&self, prev: Option<String>) {
        *self.prev.write().unwrap() = prev;
    }

    pub fn set_state(&self, state: &TaskState) {
        *self.state.write().unwrap() = state.clone();
    }
    pub(crate) fn set_start_time(&self, time: i64) {
        *self.start_time.write().unwrap() = time;
    }
    pub(crate) fn set_end_time(&self, time: i64) {
        *self.end_time.write().unwrap() = time;
    }

    pub fn set_uid(&self, uid: &str) {
        *self.uid.write().unwrap() = Some(uid.to_string());
    }

    pub(crate) fn push_back(&self, next: &str) {
        let mut children = self.children.write().unwrap();
        children.push(next.to_string());
    }

    pub fn complete(&self, ctx: &Context) {
        debug!("task::complete({:?})", self);
        self.prepare(ctx);
        if ctx.task.state().is_running() {
            self.run(ctx);
        }

        if ctx.task.state().is_next() {
            self.next(ctx);
        }
    }

    fn next(&self, ctx: &Context) {
        let node = self.node.clone();
        let children = node.children();
        debug!(
            "task::next node={:?} kind={:?} children={:?}  ctx={:?} task={:?}",
            node.id(),
            node.kind(),
            children.len(),
            ctx,
            self
        );
        if children.len() > 0 {
            for child in children {
                if self.check_cond(&child, ctx) {
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
                    while let Some(task) = parent.clone() {
                        let ctx = &ctx.proc.create_context(task.clone());
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

    fn check_cond(&self, node: &Node, ctx: &Context) -> bool {
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

    pub fn exec(&self, ctx: &Context) {
        debug!("task::exec action={:?} task={:?}", ctx.action(), self);
        self.prepare(ctx);
        if let Some(name) = ctx.action() {
            let action = EventAction::parse(&name);
            match action {
                EventAction::Create => {}
                EventAction::Complete | EventAction::Submit => {
                    if ctx.task.state().is_running() {
                        self.run(ctx);
                    }
                    if ctx.task.state().is_next() {
                        self.next(ctx);
                    }
                }
                EventAction::Back => {
                    ctx.task.set_state(&TaskState::Backed);
                    ctx.dispatch(&ctx.task, EventAction::Back);
                    if let Some(parent) = ctx.task.parent(ctx) {
                        for tid in parent.children() {
                            if let Some(task) = ctx.proc.task(&tid) {
                                if task.state().is_completed() {
                                    break;
                                }
                                task.set_uid(&ctx.uid().expect("get action uid"));
                                task.set_state(&TaskState::Skip);
                                ctx.dispatch(&ctx.task, EventAction::Skip);
                            }
                        }
                    }
                    match &ctx.options().to {
                        Some(to) => match ctx.proc.node(to) {
                            Some(node) => {
                                ctx.sched_task(&node);
                            }
                            None => {
                                ctx.task.set_state(&TaskState::Fail(format!(
                                    "not find back node by '{}'",
                                    to
                                )));
                            }
                        },
                        None => {
                            ctx.task.set_state(&TaskState::Fail(
                                "not set 'to' in back options".to_string(),
                            ));
                        }
                    }
                }
                EventAction::Cancel => {
                    if let Some(parent) = ctx.task.parent(ctx) {
                        // cancel next tasks
                        let nexts = ctx.proc.find_next_tasks(&parent.tid);
                        for n in nexts {
                            n.set_state(&TaskState::Cancelled);
                            ctx.dispatch(&n, EventAction::Cancel);
                            for tid in n.children() {
                                if let Some(task) = ctx.proc.task(&tid) {
                                    if task.state().is_completed() {
                                        break;
                                    }
                                    task.set_uid(&ctx.uid().expect("get action uid"));
                                    task.set_state(&TaskState::Cancelled);
                                    ctx.dispatch(&task, EventAction::Cancel);
                                }
                            }
                        }

                        // re-create new task
                        if let Some(node) = ctx.proc.node(&parent.nid()) {
                            ctx.sched_task(&node);
                        }
                    }
                }
                EventAction::Abort => {
                    let state = TaskState::Abort(format!("abort by uid({:?})", ctx.uid()));
                    ctx.task.set_state(&state);
                    ctx.dispatch(&ctx.task, EventAction::Abort);

                    // abort all tasks
                    let mut parent = ctx.task.parent(ctx);
                    while let Some(task) = parent {
                        let ctx = &ctx.proc.create_context(task.clone());
                        ctx.task.set_state(&state);
                        ctx.dispatch(&ctx.task, EventAction::Abort);

                        for tid in task.children() {
                            if let Some(task) = ctx.proc.task(&tid) {
                                if task.state().is_waiting() || task.state().is_running() {
                                    task.set_uid(&ctx.uid().expect("get action uid"));
                                    task.set_state(&state);
                                    ctx.dispatch(&task, EventAction::Abort);
                                }
                            }
                        }

                        parent = task.parent(ctx);
                    }
                }
                EventAction::Skip => {}
                EventAction::Error => {}
                EventAction::Custom(_) => {}
            }
        }
    }

    fn error(&self, ctx: &Context) {
        let state = ctx.task.state();
        ctx.proc.set_state(&state);
        ctx.proc.set_end_time(utils::time::time());
        let state = ctx.proc.workflow_state();
        ctx.proc.scher.evt().on_error(&state);

        // on_complete used for all complete state including error;
        ctx.proc.scher.evt().on_complete(&state);
    }
}

impl ActId for Task {
    fn tid(&self) -> String {
        self.tid.clone()
    }
}
impl ActState for Task {
    fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    fn set_state(&self, state: &TaskState) {
        *self.state.write().unwrap() = state.clone();
    }
}

#[async_trait]
impl ActTask for Task {
    fn prepare(&self, ctx: &Context) {
        debug!(
            "prepare tid={} nid={}, state={:?} vars={:?}",
            self.tid(),
            self.nid(),
            self.state(),
            ctx.vm().vars()
        );
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
            NodeData::Act(act) => {
                act.prepare(ctx);
            }
        }

        if self.state().is_none() {
            self.set_state(&TaskState::Running);
        }

        if ctx.task.state().is_error() {
            self.error(ctx);
        }
        ctx.dispatch(&ctx.task, EventAction::Create);
    }

    fn run(&self, ctx: &Context) {
        debug!(
            "run tid={} nid={} state={:?}",
            self.tid(),
            self.nid(),
            self.state()
        );
        match &self.node.data {
            NodeData::Workflow(workflow) => workflow.run(ctx),
            NodeData::Job(job) => job.run(ctx),
            NodeData::Branch(branch) => branch.run(ctx),
            NodeData::Step(step) => step.run(ctx),
            NodeData::Act(act) => {
                act.run(ctx);
            }
        }
        if ctx.task.state().is_error() {
            self.error(ctx);
        }
    }

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
            NodeData::Act(act) => {
                act.post(ctx);
            }
        }

        if self.state().is_completed() {
            ctx.dispatch(self, EventAction::Complete);
        }

        debug!(
            "post tid={} nid={}, kind={:?} state={:?} vars={:?}",
            self.tid(),
            self.nid(),
            self.node.kind(),
            self.state(),
            ctx.vm().vars()
        );
    }
}
