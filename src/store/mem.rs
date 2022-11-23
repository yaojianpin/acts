// use crate::{
//     adapter::StoreAdapter,
//     debug,
//     store::{Message, Proc, Query, Set, Task},
//     Act, ActError, ActResult, Job, ShareLock, Step, Workflow,
// };
// use std::{
//     collections::HashMap,
//     sync::{Arc, RwLock},
// };

// pub struct MemStore {
//     procs: ProcSet,
//     tasks: TaskSet,
//     messages: MessageSet,
// }

// impl MemStore {
//     pub fn new() -> Self {
//         Self {
//             procs: ProcSet {
//                 items: Arc::new(RwLock::new(HashMap::new())),
//             },
//             tasks: TaskSet {
//                 items: Arc::new(RwLock::new(HashMap::new())),
//             },
//             messages: MessageSet {
//                 items: Arc::new(RwLock::new(HashMap::new())),
//             },
//         }
//     }
// }

// impl StoreAdapter for MemStore {
//     fn init(&self) {}
//     fn procs(&self) -> Arc<dyn Set<Proc>> {
//         Arc::new(self.procs.clone())
//     }

//     fn tasks(&self) -> Arc<dyn Set<Task>> {
//         Arc::new(self.tasks.clone())
//     }

//     fn messages(&self) -> Arc<dyn Set<Message>> {
//         Arc::new(self.messages.clone())
//     }
// }

// #[derive(Clone)]
// pub struct ProcSet {
//     items: ShareLock<HashMap<String, Proc>>,
// }

// impl Set<Proc> for ProcSet {
//     fn exists(&self, id: &str) -> bool {
//         self.items.read().unwrap().contains_key(id)
//     }

//     #[inline]
//     fn find(&self, id: &str) -> Option<Proc> {
//         match self.items.read().unwrap().get(id) {
//             Some(m) => Some(m.clone()),
//             None => None,
//         }
//     }

//     fn query(&self, q: &Query, count: usize) -> Vec<Proc> {
//         let items = self.items.read().unwrap();
//         let ret: Vec<Proc> = items
//             .iter()
//             .filter(|(_, item)| predicate(item))
//             .take(count)
//             .map(|(_, item)| item.clone())
//             .collect();

//         ret
//     }

//     fn create(&self, data: &Proc) -> ActResult<bool> {
//         self.items
//             .write()
//             .unwrap()
//             .insert(data.id.clone(), data.clone());
//         Ok(true)
//     }
//     fn update(&self, data: &Proc) -> ActResult<bool> {
//         self.items
//             .write()
//             .unwrap()
//             .entry(data.id.clone())
//             .and_modify(|item| *item = data.clone());
//         Ok(true)
//     }
//     fn delete(&self, id: &str) -> ActResult<bool> {
//         self.items.write().unwrap().remove(id);
//         Ok(true)
//     }

//     fn clear(&self) {
//         self.items.write().unwrap().clear();
//     }
// }

// #[derive(Clone)]
// pub struct TaskSet {
//     items: ShareLock<HashMap<String, Task>>,
// }

// impl Set<Task> for TaskSet {
//     fn exists(&self, id: &str) -> bool {
//         self.items.read().unwrap().contains_key(id)
//     }
//     fn find(&self, id: &str) -> Option<Task> {
//         match self.items.read().unwrap().get(id) {
//             Some(m) => Some(m.clone()),
//             None => None,
//         }
//     }

//     fn query(&self, predicate: &dyn Fn(&Task) -> bool, count: usize) -> Vec<Task> {
//         let items = self.items.read().unwrap();
//         let ret = items
//             .iter()
//             .filter(|(_, item)| predicate(item))
//             .take(count)
//             .map(|(_, item)| item.clone())
//             .collect();

//         ret
//     }

//     fn create(&self, data: &Task) -> ActResult<bool> {
//         self.items
//             .write()
//             .unwrap()
//             .insert(data.id.clone(), data.clone());
//         Ok(true)
//     }
//     fn update(&self, data: &Task) -> ActResult<bool> {
//         self.items
//             .write()
//             .unwrap()
//             .entry(data.id.clone())
//             .and_modify(|item| *item = data.clone());
//         Ok(true)
//     }
//     fn delete(&self, id: &str) -> ActResult<bool> {
//         self.items.write().unwrap().remove(id);
//         Ok(true)
//     }

//     fn clear(&self) {
//         self.items.write().unwrap().clear();
//     }
// }

// #[derive(Clone)]
// pub struct MessageSet {
//     items: ShareLock<HashMap<String, Message>>,
// }

// impl Set<Message> for MessageSet {
//     fn exists(&self, id: &str) -> bool {
//         self.items.read().unwrap().contains_key(id)
//     }

//     fn find(&self, id: &str) -> Option<Message> {
//         match self.items.read().unwrap().get(id) {
//             Some(m) => Some(m.clone()),
//             None => None,
//         }
//     }

//     fn query(&self, predicate: &dyn Fn(&Message) -> bool, count: usize) -> Vec<Message> {
//         let items = self.items.read().unwrap();
//         let ret = items
//             .iter()
//             .filter(|(_, item)| predicate(item))
//             .take(count)
//             .map(|(_, item)| item.clone())
//             .collect();

//         ret
//     }

//     fn create(&self, data: &Message) -> ActResult<bool> {
//         self.items
//             .write()
//             .unwrap()
//             .insert(data.id.clone(), data.clone());
//         Ok(true)
//     }
//     fn update(&self, data: &Message) -> ActResult<bool> {
//         self.items
//             .write()
//             .unwrap()
//             .entry(data.id.clone())
//             .and_modify(|item| *item = data.clone());
//         Ok(true)
//     }
//     fn delete(&self, id: &str) -> ActResult<bool> {
//         self.items.write().unwrap().remove(id);
//         Ok(true)
//     }

//     fn clear(&self) {
//         self.items.write().unwrap().clear();
//     }
// }
