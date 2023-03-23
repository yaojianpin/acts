use crate::{
    db_debug,
    store::{data::*, DataSet, Query},
    ActError, ActResult, StoreAdapter,
};
use once_cell::sync::Lazy;
use rocksdb::{ColumnFamilyDescriptor, IteratorMode, MergeOperands, Options, DB as RocksDB};
use serde::Deserialize;
use std::sync::Arc;

static DB: Lazy<RocksDB> = Lazy::new(init);

fn init() -> RocksDB {
    db_debug!("local::init");

    let mut opts = Options::default();
    opts.create_missing_column_families(true);
    opts.create_if_missing(true);
    opts.set_max_total_wal_size(1024 * 1024);
    opts.set_merge_operator_associative("pid idx", concat_merge);

    let cf_proc = ColumnFamilyDescriptor::new("proc", opts.clone());
    let cf_task = ColumnFamilyDescriptor::new("task", opts.clone());
    let cf_message = ColumnFamilyDescriptor::new("message", opts.clone());
    let cf_model = ColumnFamilyDescriptor::new("model", opts.clone());

    let db =
        RocksDB::open_cf_descriptors(&opts, "data", vec![cf_model, cf_proc, cf_task, cf_message])
            .expect("local: init");
    db
}

fn db() -> &'static RocksDB {
    // &DB.get_or_init(init)

    &DB
}

fn mode_key(id: &str) -> Vec<u8> {
    format!("m:{}", id).as_bytes().to_vec()
}

fn idx_key<'a>(name: &str, id: &str) -> Vec<u8> {
    format!("i:{name}:{id}").as_bytes().to_vec()
}

fn get_all<T>(model_name: &str, limit: usize) -> Vec<T>
where
    for<'a> T: Deserialize<'a>,
{
    let mut ret = Vec::new();
    let db = db();
    let cf = db.cf_handle(model_name).unwrap();
    db.iterator_cf(
        cf,
        IteratorMode::From("m:".as_bytes(), rocksdb::Direction::Forward),
    )
    .take(limit)
    .for_each(|it| {
        if let Ok((_, data)) = it {
            let bytes = data.to_vec();
            let item: T = bincode::deserialize(&bytes).unwrap();
            ret.push(item);
        }
    });

    ret
}

fn find_by_idx(model_name: &str, q: &Query) -> Vec<String> {
    let db = db();
    if let Some(cf) = db.cf_handle(model_name) {
        let get_idx = |prop: &str, value: &str| {
            let idx = idx_key(prop, value);
            match db.get_cf(cf, idx) {
                Ok(ref ret) => match ret {
                    Some(data) => {
                        let list = std::str::from_utf8(&data).unwrap();
                        let ret = list
                            .trim_end_matches(",")
                            .split(",")
                            .map(|it| it.to_string())
                            .collect();

                        ret
                    }
                    None => vec![],
                },
                Err(_) => vec![],
            }
        };

        return q.predicate(get_idx);
    }

    vec![]
}

// fn update_idx(model_name: &str, prop: &str, key: &str, value: &str) {
//     let db = db();
//     let cf = db.cf_handle(model_name).unwrap();

//     let idx = idx_key(prop, key);
//     let value = &format!("{},", value);
//     db.merge_cf(cf, &idx, value).unwrap();
// }

fn concat_merge(
    _new_key: &[u8],
    existing_val: Option<&[u8]>,
    operands: &MergeOperands,
) -> Option<Vec<u8>> {
    let mut result: Vec<u8> = Vec::new();

    existing_val.map(|v| {
        for e in v {
            result.push(*e)
        }
    });
    for op in operands {
        for e in op {
            result.push(*e)
        }
    }
    Some(result)
}

#[derive(Debug)]
pub struct LocalStore {
    models: Arc<ModelSet>,
    procs: Arc<ProcSet>,
    tasks: Arc<TaskSet>,
    messages: Arc<MessageSet>,
}

impl LocalStore {
    pub fn new() -> Self {
        let db = Self {
            models: Arc::new(ModelSet {
                name: "model".to_string(),
            }),
            procs: Arc::new(ProcSet {
                name: "proc".to_string(),
            }),
            tasks: Arc::new(TaskSet {
                name: "task".to_string(),
            }),
            messages: Arc::new(MessageSet {
                name: "message".to_string(),
            }),
        };

        db.init();

        db
    }

    // #[cfg(test)]
    // pub fn is_initialized(&self) -> bool {
    //     DB.get().is_some()
    // }
}

impl StoreAdapter for LocalStore {
    fn init(&self) {}
    fn flush(&self) {
        let db = db();
        db.flush().expect("local flush data");
    }

    fn models(&self) -> Arc<dyn DataSet<Model>> {
        self.models.clone()
    }

    fn procs(&self) -> Arc<dyn DataSet<Proc>> {
        self.procs.clone()
    }

    fn tasks(&self) -> Arc<dyn DataSet<Task>> {
        self.tasks.clone()
    }

    fn messages(&self) -> Arc<dyn DataSet<Message>> {
        self.messages.clone()
    }
}

#[derive(Debug, Clone)]
pub struct ModelSet {
    name: String,
}

impl DataSet<Model> for ModelSet {
    fn exists(&self, id: &str) -> bool {
        db_debug!("local::Model.exists({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(_) => true,
                None => false,
            },
            Err(_) => false,
        }
    }

    fn find(&self, id: &str) -> Option<Model> {
        db_debug!("local::Model.find({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(data) => {
                    let model: Model = bincode::deserialize(data.as_ref()).unwrap();
                    Some(model)
                }
                None => None,
            },
            Err(_err) => None,
        }
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Model>> {
        db_debug!("local::Model.query({:?})", q);
        let mut ret = Vec::new();
        let mut limit = q.limit();
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }
        if q.is_cond() {
            for id in find_by_idx(&self.name, q) {
                if let Some(it) = self.find(&id) {
                    ret.push(it);
                }
            }
        } else {
            ret = get_all(&self.name, limit);
        }

        Ok(ret)
    }

    fn create(&self, model: &Model) -> ActResult<bool> {
        db_debug!("local::Model.create({})", proc.id);
        let data = bincode::serialize(model).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&model.id), data) {
            Ok(_) => {
                // let idx = idx_key("id", &proc.id);
                // let value = &format!("{},", proc.id);
                // db.merge_cf(cf, &idx, value).unwrap();
                Ok(true)
            }
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn update(&self, model: &Model) -> ActResult<bool> {
        db_debug!("local::Model.update({})", proc.id);
        let data = bincode::serialize(model).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&model.id), data) {
            Ok(_) => Ok(true),
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        db_debug!("local::Model.delete({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();

        match self.find(id) {
            Some(item) => match db.delete_cf(cf, mode_key(id)) {
                Ok(_) => {
                    let idx = idx_key("id", &item.id);
                    db.delete_cf(cf, &idx).unwrap();
                    Ok(true)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            },
            None => Err(ActError::StoreError(format!(
                "can not find the key: {}",
                id
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcSet {
    name: String,
}

impl DataSet<Proc> for ProcSet {
    fn exists(&self, id: &str) -> bool {
        db_debug!("local::Proc.exists({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(_) => true,
                None => false,
            },
            Err(_) => false,
        }
    }

    fn find(&self, id: &str) -> Option<Proc> {
        db_debug!("local::Proc.find({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(data) => {
                    let proc: Proc = bincode::deserialize(data.as_ref()).unwrap();
                    Some(proc)
                }
                None => None,
            },
            Err(_err) => None,
        }
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Proc>> {
        db_debug!("local::Proc.query({:?})", q);
        let mut ret = Vec::new();
        let mut limit = q.limit();
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }
        if q.is_cond() {
            for id in find_by_idx(&self.name, q) {
                if let Some(it) = self.find(&id) {
                    ret.push(it);
                }
            }
        } else {
            ret = get_all(&self.name, limit);
        }

        Ok(ret)
    }

    fn create(&self, proc: &Proc) -> ActResult<bool> {
        db_debug!("local::Proc.create({})", proc.id);
        let data = bincode::serialize(proc).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&proc.id), data) {
            Ok(_) => {
                let idx = idx_key("pid", &proc.pid);
                let value = &format!("{},", proc.id);
                db.merge_cf(cf, &idx, value).unwrap();

                Ok(true)
            }
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn update(&self, proc: &Proc) -> ActResult<bool> {
        db_debug!("local::Proc.update({})", proc.id);
        let data = bincode::serialize(proc).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&proc.id), data) {
            Ok(_) => Ok(true),
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        db_debug!("local::Proc.delete({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();

        match self.find(id) {
            Some(item) => match db.delete_cf(cf, mode_key(id)) {
                Ok(_) => {
                    let idx = idx_key("pid", &item.pid);
                    db.delete_cf(cf, &idx).unwrap();
                    Ok(true)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            },
            None => Err(ActError::StoreError(format!(
                "can not find the key: {}",
                id
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskSet {
    name: String,
}

impl DataSet<Task> for TaskSet {
    fn exists(&self, id: &str) -> bool {
        db_debug!("local::Task.exists({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(_) => true,
                None => false,
            },
            Err(_) => false,
        }
    }

    fn find(&self, id: &str) -> Option<Task> {
        db_debug!("local::Task.find({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(data) => {
                    let task: Task = bincode::deserialize(data.as_ref()).unwrap();
                    Some(task)
                }
                None => None,
            },
            Err(_err) => None,
        }
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Task>> {
        db_debug!("local::Task.query({:?})", q);
        let mut ret = Vec::new();
        let mut limit = q.limit();

        #[allow(unused_assignments)]
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }
        if q.is_cond() {
            for id in find_by_idx(&self.name, q) {
                if let Some(it) = self.find(&id) {
                    ret.push(it);
                }
            }
        } else {
            ret = get_all(&self.name, q.limit());
        }

        Ok(ret)
    }

    fn create(&self, task: &Task) -> ActResult<bool> {
        db_debug!("local::Task.create({})", task.id);
        let data = bincode::serialize(task).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&task.id), data) {
            Ok(_) => {
                let idx_key = idx_key("pid", &task.pid);
                let value = &format!("{},", task.id);
                db.merge_cf(cf, &idx_key, value).unwrap();
                Ok(true)
            }
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn update(&self, task: &Task) -> ActResult<bool> {
        db_debug!("local::Task.update({})", task.id);
        let data = bincode::serialize(task).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&task.id), data) {
            Ok(_) => Ok(true),
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        db_debug!("local::Task.delete({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match self.find(id) {
            Some(item) => match db.delete_cf(cf, mode_key(id)) {
                Ok(_) => {
                    let idx = idx_key("pid", &item.pid);
                    db.delete_cf(cf, &idx).unwrap();
                    Ok(true)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            },
            None => Err(ActError::StoreError(format!(
                "can not find the key: {}",
                id
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessageSet {
    name: String,
}

impl DataSet<Message> for MessageSet {
    fn exists(&self, id: &str) -> bool {
        db_debug!("local::Message.delete({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(_) => true,
                None => false,
            },
            Err(_) => false,
        }
    }

    fn find(&self, id: &str) -> Option<Message> {
        db_debug!("local::Message.find({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.get_cf(cf, mode_key(id)) {
            Ok(opt) => match opt {
                Some(data) => {
                    let msg: Message = bincode::deserialize(data.as_ref()).unwrap();
                    Some(msg)
                }
                None => None,
            },
            Err(_err) => None,
        }
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Message>> {
        db_debug!("local::Message.find({:?})", q);
        let mut ret = Vec::new();
        let mut limit = q.limit();

        #[allow(unused_assignments)]
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }
        if q.is_cond() {
            for id in find_by_idx(&self.name, q) {
                if let Some(it) = self.find(&id) {
                    ret.push(it);
                }
            }
        } else {
            ret = get_all(&self.name, q.limit());
        }

        Ok(ret)
    }

    fn create(&self, msg: &Message) -> ActResult<bool> {
        db_debug!("local::Message.create({})", msg.id);
        let data = bincode::serialize(msg).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&msg.id), data) {
            Ok(_) => {
                let idx_key = idx_key("pid", &msg.pid);
                let value = &format!("{},", msg.id);
                db.merge_cf(cf, &idx_key, value).unwrap();
                Ok(true)
            }
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn update(&self, msg: &Message) -> ActResult<bool> {
        db_debug!("local::Message.update({})", msg.id);
        let data = bincode::serialize(msg).unwrap();
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match db.put_cf(cf, mode_key(&msg.id), data) {
            Ok(_) => {
                db.flush().unwrap();
                Ok(true)
            }
            Err(err) => Err(ActError::StoreError(err.to_string())),
        }
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        db_debug!("local::Message.delete({})", id);
        let db = db();
        let cf = db.cf_handle(&self.name).unwrap();
        match self.find(id) {
            Some(item) => match db.delete_cf(cf, mode_key(id)) {
                Ok(_) => {
                    let idx = idx_key("pid", &item.pid);
                    db.delete_cf(cf, &idx).unwrap();
                    Ok(true)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            },
            None => Err(ActError::StoreError(format!(
                "can not find the key: {}",
                id
            ))),
        }
    }
}
