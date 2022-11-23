// use sled::{Db, Tree};
// use std::sync::Arc;

// use super::{data::*, Set};
// use crate::{ActResult, StoreAdapter};
// use tokio::sync::OnceCell;

// static DB: OnceCell<Db> = OnceCell::const_new();

// fn db<'a>() -> &'static Db {
//     DB.get().unwrap()
// }

// #[cfg(test)]
// pub fn clear() {
//     let db = db();
// }

// pub struct LocalStore {
//     procs: Arc<ProcSet>,
//     tasks: Arc<TaskSet>,
//     messages: Arc<MessageSet>,
// }

// impl LocalStore {
//     pub fn new() -> Self {
//         let db = DB.get_or_init(|| sled::open("data").unwrap());
//         Self {
//             _db: db.clone(),
//             procs: Arc::new(ProcSet {
//                 name: "p".to_string(),
//                 items: db.open_tree("p").unwrap(),
//             }),
//             tasks: Arc::new(TaskSet {
//                 name: "t".to_string(),
//                 items: db.open_tree("t").unwrap(),
//             }),
//             messages: Arc::new(MessageSet {
//                 name: "m".to_string(),
//                 items: db.open_tree("m").unwrap(),
//             }),
//         }
//     }

//     #[cfg(test)]
//     pub fn is_initialized(&self) -> bool {
//         DB.initialized()
//     }
// }

// impl StoreAdapter for LocalStore {
//     fn init(&self) {
//         let db = sled::open("data").unwrap();
//     }

//     fn procs(&self) -> Arc<dyn Set<Proc>> {
//         self.procs.clone()
//     }

//     fn tasks(&self) -> Arc<dyn Set<Task>> {
//         self.tasks.clone()
//     }

//     fn messages(&self) -> Arc<dyn Set<Message>> {
//         self.messages.clone()
//     }

//     fn clear(&self) {
//         self._db.flush().unwrap();
//         self._db.clear().unwrap();
//     }
// }

// #[derive(Clone)]
// pub struct ProcSet {
//     name: String,
//     items: Tree,
// }

// impl Set<Proc> for ProcSet {
//     fn exists(&self, id: &str) -> bool {
//         self.items.contains_key(id).is_ok()
//     }

//     fn find(&self, id: &str) -> Option<Proc> {
//         if let Ok(item) = self.items.get(id) {
//             if let Some(data) = item {
//                 let proc: Proc = bincode::deserialize(data.as_ref()).unwrap();
//                 return Some(proc);
//             }
//         }

//         None
//     }

//     fn query(&self, predicate: &dyn Fn(&Proc) -> bool, count: usize) -> Vec<Proc> {
//         self.items
//             .iter()
//             .map(|item| {
//                 let (_, data) = item.unwrap();
//                 let proc: Proc = bincode::deserialize(data.as_ref()).unwrap();
//                 proc
//             })
//             .filter(|proc| {
//                 return predicate(&proc);
//             })
//             .take(count)
//             .collect()
//     }

//     fn create(&self, proc: &Proc) -> ActResult<bool> {
//         let data = bincode::serialize(proc).unwrap();
//         self.items.insert(&proc.id, data).unwrap();
//         Ok(true)
//     }
//     fn update(&self, proc: &Proc) -> ActResult<bool> {
//         self.items
//             .fetch_and_update(&proc.id, move |_item| {
//                 let data = bincode::serialize(proc).unwrap();
//                 Some(data)
//             })
//             .unwrap();

//         Ok(true)
//     }
//     fn delete(&self, id: &str) -> ActResult<bool> {
//         self.items.remove(id).unwrap();
//         Ok(true)
//     }
//     fn clear(&self) {
//         self.items.clear().unwrap();
//     }
// }

// #[derive(Clone)]
// pub struct TaskSet {
//     name: String,
//     items: Tree,
// }

// impl Set<Task> for TaskSet {
//     fn exists(&self, id: &str) -> bool {
//         self.items.contains_key(id).is_ok()
//     }
//     fn find(&self, id: &str) -> Option<Task> {
//         if let Ok(item) = self.items.get(id) {
//             if let Some(data) = item {
//                 let task: Task = bincode::deserialize(data.as_ref()).unwrap();
//                 return Some(task);
//             }
//         }

//         None
//     }
//     fn query(&self, predicate: &dyn Fn(&Task) -> bool, count: usize) -> Vec<Task> {
//         self.items
//             .iter()
//             .map(|item| {
//                 let (_, data) = item.unwrap();
//                 let proc: Task = bincode::deserialize(data.as_ref()).unwrap();
//                 proc
//             })
//             .filter(|task| {
//                 return predicate(&task);
//             })
//             .take(count)
//             .collect()
//     }

//     fn create(&self, task: &Task) -> ActResult<bool> {
//         let data = bincode::serialize(task).unwrap();
//         self.items.insert(&task.id, data).unwrap();
//         Ok(true)
//     }
//     fn update(&self, task: &Task) -> ActResult<bool> {
//         self.items
//             .fetch_and_update(&task.id, move |_item| {
//                 let data = bincode::serialize(task).unwrap();
//                 Some(data)
//             })
//             .unwrap();

//         Ok(true)
//     }
//     fn delete(&self, id: &str) -> ActResult<bool> {
//         self.items.remove(id).unwrap();
//         Ok(true)
//     }
//     fn clear(&self) {
//         self.items.clear().unwrap();
//     }
// }

// #[derive(Clone)]
// pub struct MessageSet {
//     name: String,
//     items: Tree,
// }

// impl Set<Message> for MessageSet {
//     fn exists(&self, id: &str) -> bool {
//         self.items.contains_key(id).is_ok()
//     }

//     fn find(&self, id: &str) -> Option<Message> {
//         if let Ok(item) = self.items.get(id) {
//             if let Some(data) = item {
//                 let proc: Message = bincode::deserialize(data.as_ref()).unwrap();
//                 return Some(proc);
//             }
//         }

//         None
//     }

//     fn query(&self, predicate: &dyn Fn(&Message) -> bool, count: usize) -> Vec<Message> {
//         self.items
//             .iter()
//             .map(|item| {
//                 let (_, data) = item.unwrap();
//                 let proc: Message = bincode::deserialize(data.as_ref()).unwrap();
//                 proc
//             })
//             .filter(|msg| {
//                 return predicate(&msg);
//             })
//             .take(count)
//             .collect()
//     }

//     fn create(&self, msg: &Message) -> ActResult<bool> {
//         let data = bincode::serialize(msg).unwrap();
//         self.items.insert(&msg.id, data).unwrap();
//         Ok(true)
//     }
//     fn update(&self, msg: &Message) -> ActResult<bool> {
//         self.items
//             .fetch_and_update(&msg.id, move |_item| {
//                 let data = bincode::serialize(msg).unwrap();
//                 Some(data)
//             })
//             .unwrap();

//         Ok(true)
//     }
//     fn delete(&self, id: &str) -> ActResult<bool> {
//         self.items.remove(id).unwrap();
//         Ok(true)
//     }
//     fn clear(&self) {
//         self.items.clear().unwrap();
//     }
// }
