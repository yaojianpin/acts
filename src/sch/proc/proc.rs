use crate::{
    debug,
    env::VirtualMachine,
    sch::{
        event::{EventAction, EventData, Message},
        proc::{
            tree,
            tree::{Node, Tree},
        },
        ActState, ActTask, Context, Scheduler, Task, TaskState,
    },
    utils, ShareLock, Vars, Workflow,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Clone)]
pub struct Proc {
    pub(in crate::sch) vm: Arc<VirtualMachine>,
    pub(in crate::sch) scher: Arc<Scheduler>,
    pub(in crate::sch) tree: Arc<Tree<Task>>,

    pid: String,
    workflow: Arc<Workflow>,
    state: ShareLock<TaskState>,
    ctx: ShareLock<Option<Context>>,
    messages: ShareLock<HashMap<String, Message>>,

    sync: Arc<Mutex<i32>>,
}

impl Proc {
    pub fn new(scher: Arc<Scheduler>, workflow: &Workflow, state: &TaskState) -> Self {
        // set the pid with biz_id by default
        let mut pid = workflow.biz_id();
        if pid.is_empty() {
            pid = utils::longid();
        }

        let vm = scher.env().vm();
        let vars = utils::fill_vars(&vm, &workflow.env);
        Proc::new_raw(scher, workflow, &pid, state, &vars)
    }

    pub fn new_raw(
        scher: Arc<Scheduler>,
        workflow: &Workflow,
        pid: &str,
        state: &TaskState,
        vars: &Vars,
    ) -> Self {
        let vm = scher.env().vm();
        vm.append(vars.clone());

        let mut workflow = workflow.clone();
        let tr = tree::from(&mut workflow);
        Proc {
            pid: pid.to_string(),
            vm: Arc::new(vm),
            scher,
            workflow: Arc::new(workflow.clone()),
            tree: tr,
            state: Arc::new(RwLock::new(state.clone())),
            messages: Arc::new(RwLock::new(HashMap::new())),
            ctx: Arc::new(RwLock::new(None)),
            sync: Arc::new(Mutex::new(0)),
        }
    }

    pub fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    pub fn pid(&self) -> String {
        self.pid.clone()
    }

    pub fn workflow(&self) -> Arc<Workflow> {
        self.workflow.clone()
    }

    pub fn task(&self, tid: &str) -> Option<Task> {
        match self.tree.node(tid) {
            Some(node) => Some(node.data()),
            None => None,
        }
    }

    pub fn vm(&self) -> Arc<VirtualMachine> {
        self.vm.clone()
    }

    pub fn context(&self) -> Arc<Context> {
        let mut ctx = self.ctx.write().unwrap();
        if ctx.is_none() {
            *ctx = Some(Context::new(self));
        }

        let ctx = ctx.clone().unwrap();
        Arc::new(ctx)
    }

    pub fn message(&self, id: &str) -> Option<Message> {
        match self.messages.read().unwrap().get(id) {
            Some(m) => Some(m.clone()),
            None => None,
        }
    }

    pub fn message_by_uid(&self, uid: &str) -> Option<Message> {
        let mut ret = None;
        let messages = &*self.messages.read().unwrap();
        for (_, m) in messages {
            if m.user == uid {
                ret = Some(m.clone());
                break;
            }
        }

        ret
    }

    // pub fn needs(&self) -> Vec<String> {
    //     self.job.needs.clone()
    // }

    // pub fn workflow_id(&self) -> String {
    //     self.job.workflow().id.clone()
    // }

    pub fn set_state(&self, state: &TaskState) {
        *self.state.write().unwrap() = state.clone();
        self.workflow.set_state(state);
    }

    pub async fn run_with_task(&self, tid: &str) {
        debug!("sch::proc::run_with_task(tid={})", tid);
        let opt = self.tree.node(tid);
        if let Some(node) = opt {
            let task = node.data();

            let ctx = &self.context();

            task.prepare(ctx);
            task.run(ctx);
            task.next(ctx);
            // self.next(&node, ctx);
        }
    }

    pub fn run_with_message(&self, msg: &Message) {
        debug!("sch::proc::run_with_message(id={})", msg.id);
        let opt = self.tree.node(&msg.tid);
        if let Some(node) = opt {
            let task = node.data();

            let ctx = &self.context();
            ctx.set_message(msg);

            let mut count = self.sync.lock().unwrap();
            task.prepare(ctx);
            task.run(ctx);
            task.next(ctx);
            *count += 1;
        }
    }

    pub fn start(&self) {
        let task = self.clone();
        let tr = self.tree.clone();

        #[cfg(feature = "debug")]
        tr.print();

        self.set_state(&TaskState::Running);
        let data = EventData {
            pid: self.pid.clone(),
            state: self.state(),
            action: EventAction::Create,
            vars: HashMap::new(),
        };
        self.scher.evt().disp_proc(self, &data);
        if let Some(root) = &tr.root {
            self.scher.sched_task(&task, &root.id());
        }
    }

    pub fn complete(&self) {
        let data = EventData {
            pid: self.pid.clone(),
            state: self.state(),
            action: EventAction::Next,
            vars: self.workflow.outputs(),
        };
        self.scher.evt().disp_proc(self, &data);
    }

    pub fn next(&self, node: &Arc<Node<Task>>, ctx: &Context) {
        debug!("next: {}->", node.id());
        let state = node.data().state();
        match state {
            TaskState::Pending
            | TaskState::WaitingEvent
            | TaskState::Fail(..)
            | TaskState::Abort(..) => {
                debug!("      post");

                // if the node is fail or pending or waiting for event
                // only post data
                node.data().post(ctx);
            }
            TaskState::Skip => {
                self.next_inner(node, ctx);
            }
            _ => {
                let children = node.children();
                if children.len() > 0 {
                    let next = &children[0];
                    debug!("      {}", next.id());
                    next.data().set_state(&TaskState::None);
                    self.scher.sched_task(&self.clone(), &next.id());
                } else {
                    node.data().post(ctx);
                    self.next_inner(node, ctx);
                }
            }
        }
    }

    fn next_inner(&self, node: &Arc<Node<Task>>, ctx: &Context) {
        let next = node.next().upgrade();
        match next {
            Some(next) => {
                debug!("      {}", next.id());

                // found the next node and schedule
                next.data().set_state(&TaskState::None);
                self.scher.sched_task(&self.clone(), &next.id());
            }
            None => {
                debug!("      next=None");

                let mut parent = node.parent();
                while let Some(p) = &parent {
                    // run post for the parent node
                    p.data().post(ctx);

                    // find the next node
                    let n = p.next().upgrade();
                    match n {
                        Some(next) => {
                            // this one is the next node of the parent
                            next.data().set_state(&TaskState::None);
                            self.scher.sched_task(&self.clone(), &next.id());
                            break;
                        }
                        None => {
                            // continue to find parent node
                            parent = p.parent();
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn make_message(&self, tid: &str, user: &str) -> Message {
        let msg = Message::new(&self.pid, tid, user, None);
        debug!("sch::proc::make_message(id={}, tid={})", msg.id, tid);
        self.messages
            .write()
            .unwrap()
            .insert(tid.to_string(), msg.clone());

        msg
    }
}
