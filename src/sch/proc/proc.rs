use crate::{
    debug,
    env::VirtualMachine,
    sch::{
        consts::EVT_CANCEL,
        event::{Message, UserMessage},
        tree::TaskTree,
        tree::{Node, NodeTree},
        Context, Scheduler, Task, TaskState,
    },
    utils, ProcInfo, ShareLock, State, Vars, Workflow,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Clone)]
pub struct Proc {
    pub(in crate::sch) vm: Arc<VirtualMachine>,
    pub(in crate::sch) scher: Arc<Scheduler>,
    pub(in crate::sch) tree: Arc<NodeTree>,

    pid: String,
    workflow: Arc<Workflow>,

    messages: ShareLock<HashMap<String, Message>>,
    tasks: ShareLock<TaskTree>,

    state: ShareLock<TaskState>,
    start_time: ShareLock<i64>,
    end_time: ShareLock<i64>,

    sync: Arc<Mutex<i32>>,
}

impl Proc {
    pub fn new(pid: &str, scher: Arc<Scheduler>, workflow: &Workflow, state: &TaskState) -> Self {
        // set the pid with biz_id by default
        // let mut pid = workflow.biz_id.clone();
        // if pid.is_empty() {
        //     pid = utils::longid();
        // }

        let vm = scher.env().vm();
        let vars = utils::fill_vars(&vm, &workflow.env);
        Proc::new_raw(scher, workflow, pid, state, &vars)
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
        let tr = NodeTree::build(&mut workflow);
        Proc {
            pid: pid.to_string(),
            vm: Arc::new(vm),
            scher,
            workflow: Arc::new(workflow.clone()),
            tree: tr,
            state: Arc::new(RwLock::new(state.clone())),
            start_time: Arc::new(RwLock::new(0)),
            end_time: Arc::new(RwLock::new(0)),
            messages: Arc::new(RwLock::new(HashMap::new())),
            tasks: Arc::new(RwLock::new(TaskTree::new())),
            sync: Arc::new(Mutex::new(0)),
        }
    }

    pub fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    pub fn start_time(&self) -> i64 {
        *self.start_time.read().unwrap()
    }
    pub fn end_time(&self) -> i64 {
        *self.end_time.read().unwrap()
    }

    pub fn outputs(&self) -> Vars {
        let outputs = &self.workflow().outputs;
        let vars = utils::fill_vars(&self.vm, outputs);
        vars
    }

    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time() - self.start_time();
        }

        0
    }

    pub fn workflow_state(&self) -> State<Workflow> {
        State {
            pid: self.pid(),
            node: self.workflow(),
            state: self.state(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            outputs: self.outputs(),
        }
    }

    pub fn pid(&self) -> String {
        self.pid.clone()
    }

    pub fn workflow(&self) -> Arc<Workflow> {
        self.workflow.clone()
    }

    pub fn info(&self) -> ProcInfo {
        let workflow = self.workflow();
        ProcInfo {
            pid: self.pid.clone(),
            name: workflow.name.clone(),
            model_id: workflow.id.clone(),
            state: self.state(),
            start_time: self.start_time(),
            end_time: self.end_time(),
            vars: self.vm().vars(),
        }
    }
    pub fn task(&self, tid: &str) -> Option<Arc<Task>> {
        self.tasks.read().unwrap().task_by_tid(tid)
    }

    pub fn find_next_tasks(&self, tid: &str) -> Vec<Arc<Task>> {
        let tasks = self.tasks.read().unwrap();
        tasks.find_next_tasks(tid)
    }

    pub fn node(&self, nid: &str) -> Option<Arc<Node>> {
        self.tree.node(nid)
    }

    pub fn children(&self, task: &Task) -> Vec<Arc<Task>> {
        let mut ret = Vec::new();

        let tasks = self.tasks.read().unwrap();
        for tid in task.children() {
            if let Some(t) = tasks.task_by_tid(&tid) {
                ret.push(t);
            }
        }
        ret
    }

    pub fn task_by_nid(&self, nid: &str) -> Vec<Arc<Task>> {
        self.tasks.read().unwrap().task_by_nid(nid)
    }

    pub fn vm(&self) -> Arc<VirtualMachine> {
        self.vm.clone()
    }

    pub fn create_context(&self, task: Arc<Task>) -> Arc<Context> {
        let ctx = Context::new(self, task);
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
            if let Some(id) = &m.uid {
                if uid == id {
                    ret = Some(m.clone());
                    break;
                }
            }
        }

        ret
    }

    pub fn task_by_uid(&self, uid: &str, state: TaskState) -> Vec<Arc<Task>> {
        let tasks = &*self.tasks.read().unwrap();
        tasks.task_by_uid(uid, state)
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

    pub fn do_message(&self, msg: &UserMessage) {
        debug!("do_message msg={:?}", msg);
        let mut count = self.sync.lock().unwrap();
        let tasks = if msg.action == EVT_CANCEL {
            self.task_by_uid(&msg.uid, TaskState::Success)
        } else {
            self.task_by_uid(&msg.uid, TaskState::WaitingEvent)
        };
        if tasks.len() > 0 {
            // executes only one task every time
            if let Some(task) = tasks.get(0) {
                let ctx = &self.create_context(task.clone());
                ctx.set_message(msg);
                task.exec(ctx);
            }
        }
        *count += 1;
    }

    pub fn do_task(&self, tid: &str) {
        debug!("do_task pid={} tid={}", self.pid, tid);
        let mut count = self.sync.lock().unwrap();

        if let Some(task) = self.task(tid) {
            if !task.state().is_completed() {
                let ctx = self.create_context(task.clone());
                task.complete(&ctx);
            }
        }
        *count += 1;
    }

    pub fn start(&self) {
        debug!("proc::start({})", self.pid);
        let mut count = self.sync.lock().unwrap();
        self.scher.cache().push(self);

        let tr = self.tree.clone();
        self.set_state(&TaskState::Running);
        if let Some(root) = &tr.root {
            let task = self.create_task(root, None);
            self.scher.sched_task(&task);
        }
        *count += 1;
    }

    pub(crate) fn make_message(&self, tid: &str, uid: Option<String>, vars: Vars) -> Message {
        let msg = Message::new(&self.pid, tid, uid, vars);
        debug!("sch::proc::make_message(id={}, tid={})", msg.id, tid);
        self.messages
            .write()
            .unwrap()
            .insert(tid.to_string(), msg.clone());

        msg
    }

    pub fn create_task(&self, node: &Arc<Node>, prev: Option<Arc<Task>>) -> Arc<Task> {
        let task = Arc::new(Task::new(&self.pid, &utils::shortid(), node.clone()));

        if let Some(prev) = prev {
            task.set_prev(Some(prev.tid.clone()));
            prev.push_back(&task.tid);
        }

        self.push_task(task.clone());

        task
    }

    pub fn push_task(&self, task: Arc<Task>) {
        let mut tasks = self.tasks.write().unwrap();
        tasks.push(task);
    }
}
