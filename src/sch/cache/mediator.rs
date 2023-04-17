// use crate::{
//     event::Message,
//     sch::{self, NodeKind, Scheduler, TaskState},
//     store::{self, Store},
//     utils::{self, Id},
//     ActResult, Vars, Workflow,
// };
// use std::sync::Arc;

// pub struct StoreMediator {
//     store: Arc<Store>,
// }

// impl StoreMediator {
//     pub fn new(store: Arc<Store>) -> Self {
//         Self { store }
//     }

//     pub fn load(&self, scher: Arc<Scheduler>, cap: usize) -> Vec<Arc<sch::Proc>> {
//         let mut ret = Vec::new();
//         if cap > 0 {
//             let procs = self.store.procs(cap).expect("get store procs");
//             for p in procs {
//                 let workflow = Workflow::from_str(&p.model).unwrap();
//                 let vars = &utils::vars::from_string(&p.vars);
//                 let state = p.state.clone();
//                 let mut proc =
//                     sch::Proc::new_raw(scher.clone(), &workflow, &p.pid, &state.into(), vars);

//                 self.load_tasks(&mut proc);
//                 self.load_messages(&mut proc);

//                 ret.push(Arc::new(proc));
//             }
//         }

//         ret
//     }

//     fn load_tasks(&self, proc: &mut sch::Proc) {
//         let tasks = self.store.tasks(&proc.pid()).expect("get proc tasks");
//         for t in tasks {
//             let kind: NodeKind = t.kind.into();
//             let state: TaskState = t.state.into();
//             if kind == NodeKind::Workflow {
//                 proc.set_state(&state);
//             }

//             if let Some(node) = proc.node(&t.nid) {
//                 let task = sch::Task::new(&proc, &t.tid, node);
//                 task.set_state(&state);
//                 task.set_start_time(t.start_time);
//                 task.set_end_time(t.end_time);
//                 if !t.uid.is_empty() {
//                     task.set_uid(&t.uid);
//                 }
//                 proc.push_task(Arc::new(task));
//             }
//         }
//     }

//     fn load_messages(&self, proc: &mut sch::Proc) {
//         let messages = self.store.messages(&proc.pid()).expect("get proc tasks");
//         for m in messages {
//             let uid = if m.uid.is_empty() { None } else { Some(m.uid) };
//             proc.make_message(&m.tid, uid, utils::vars::from_string(&m.vars));
//         }
//     }

//     pub fn create_proc(&self, proc: &sch::Proc) {
//         let workflow = &*proc.workflow();
//         let data = store::Proc {
//             id: proc.pid(), // pid is global unique id
//             pid: proc.pid(),
//             model: serde_yaml::to_string(workflow).unwrap(),
//             state: proc.state().into(),
//             start_time: proc.start_time(),
//             end_time: proc.end_time(),
//             vars: utils::vars::to_string(&proc.vm().vars()),
//         };
//         self.store.create_proc(&data).expect("create proc");
//     }

//     pub fn load_proc(&self, pid: &str, scher: &Scheduler) -> Option<Arc<sch::Proc>> {
//         match self.store.proc(pid) {
//             Ok(p) => {
//                 let workflow = Workflow::from_str(&p.model).unwrap();
//                 let mut proc = scher.create_raw_proc(pid, &workflow);
//                 proc.set_state(&p.state.into());
//                 self.load_tasks(&mut proc);
//                 self.load_messages(&mut proc);
//                 return Some(Arc::new(proc));
//             }
//             Err(_) => None,
//         }
//     }

//     pub fn update_proc(&self, proc: &sch::Proc) {
//         let workflow = &*proc.workflow();
//         let proc = store::Proc {
//             id: proc.pid(), // pid is global unique id
//             pid: proc.pid(),
//             model: serde_yaml::to_string(workflow).unwrap(),
//             state: proc.state().into(),
//             start_time: proc.start_time(),
//             end_time: proc.end_time(),
//             vars: utils::vars::to_string(&proc.vm().vars()),
//         };
//         self.store.update_proc(&proc).expect("update store proc");
//     }

//     pub fn create_message(&self, msg: &Message) {
//         let uid = match &msg.uid {
//             Some(uid) => uid,
//             None => "",
//         };
//         self.store
//             .create_message(&store::Message {
//                 id: msg.id.clone(),
//                 pid: msg.pid.clone(),
//                 tid: msg.tid.clone(),
//                 uid: uid.to_string(),
//                 vars: utils::vars::to_string(&msg.vars),
//                 create_time: msg.create_time,
//                 update_time: msg.update_time,
//                 state: msg.state.clone().into(),
//             })
//             .expect("create proc message");
//     }

//     pub fn create_task(&self, task: &sch::Task) {
//         let tid = &task.tid;
//         let nid = task.nid();
//         let id = Id::new(&task.pid, tid);
//         let task = store::Task {
//             id: id.id(),
//             kind: task.node.kind().to_string(),
//             pid: task.pid.clone(),
//             tid: tid.clone(),
//             nid: nid,
//             state: task.state().into(),
//             start_time: task.start_time(),
//             end_time: task.end_time(),
//             uid: match task.uid() {
//                 Some(u) => u,
//                 None => "".to_string(),
//             },
//         };
//         self.store.create_task(&task).expect("store: create task");
//     }

//     pub fn update_task(&self, task: &sch::Task, vars: &Vars) {
//         let pid = &task.pid;

//         let mut proc = self.store.proc(pid).expect("get store proc");
//         proc.vars = utils::vars::to_string(vars);
//         self.store
//             .update_proc(&proc)
//             .expect("update store proc vars");

//         let state = task.state();
//         let mut task = self.store.task(pid, &task.tid).expect("get store task");
//         task.state = state.into();
//         self.store.update_task(&task).expect("update store task");
//     }

//     pub fn remove_proc(&self, pid: &str) -> ActResult<bool> {
//         self.store.remove_proc(pid)
//     }

//     pub fn flush(&self) {
//         self.store.flush();
//     }
// }
