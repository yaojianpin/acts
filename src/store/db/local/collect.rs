use super::database::Database;
use crate::{
    store::{data::*, DbSet, Query},
    ActResult, ShareLock,
};
use acts_tag::Value;
use rocksdb::{Direction, IteratorMode};
use std::collections::HashMap;
use tracing::trace;

#[derive(Debug)]
pub struct Collect<T> {
    db: ShareLock<Database>,
    name: String,
    _t: Vec<T>,
}

fn get_all<T: DbModel>(db: &Database, model_name: &str, limit: usize) -> Vec<T> {
    let mut ret = Vec::new();
    let cf = db.cf_handle(model_name).unwrap();
    let mut count = 0;
    db.iterator_cf(&cf, IteratorMode::End)
        .take(limit)
        .for_each(|it| {
            let bytes = it.value;
            if let Ok(model) = T::from_slice(&bytes) {
                ret.push(model);
            }
            count += 1;
        });

    ret
}

fn get_by_keys<T: DbModel>(db: &Database, model_name: &str, keys: &[Box<[u8]>]) -> Vec<T> {
    let mut ret = Vec::new();
    let cf = db.cf_handle(model_name).unwrap();
    keys.into_iter().for_each(|it| {
        if let Ok(data) = db.get_cf(&cf, it) {
            if let Ok(model) = T::from_slice(&data) {
                ret.push(model);
            }
        }
    });

    ret
}

impl<T> Collect<T>
where
    T: DbModel,
{
    pub fn new(db: &ShareLock<Database>, name: &str) -> Self {
        db.write().unwrap().init_cfs(name, &T::keys());
        Self {
            db: db.clone(),
            name: name.to_string(),
            _t: Vec::new(),
        }
    }
}

impl<T> Collect<T>
where
    T: DbModel,
{
    fn exists(&self, id: &str) -> ActResult<bool> {
        trace!("local::{}.exists({})", self.name, id);
        let db = self.db.read().unwrap();
        let cf = db.cf_handle(&self.name).unwrap();
        let ret = db.get_cf(&cf, id.as_bytes());

        Ok(ret.is_ok())
    }

    fn find(&self, id: &str) -> ActResult<T> {
        trace!("local::{}.find({})", self.name, id);
        let db = self.db.read().unwrap();
        let cf = db.cf_handle(&self.name)?;
        let data = db.get_cf(&cf, id.as_bytes())?;
        let model = T::from_slice(&data)?;
        Ok(model)
    }

    fn query(&self, q: &Query) -> ActResult<Vec<T>> {
        trace!("local::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        let mut limit = q.limit();
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }

        let ret = if q.is_cond() {
            let mut key_count: HashMap<Box<[u8]>, usize> = HashMap::new();
            let mut query_results: Vec<Box<[u8]>> = Vec::new();
            let cond_len = q.queries().len();
            for (qk, qv) in &q.queries() {
                let cf = db.cf_idx_handle(&self.name, &qk)?;

                let value = Value::from(qv).unwrap();
                let mut iter =
                    db.iterator_cf(&cf, IteratorMode::From(value.data(), Direction::Forward));

                while let Some(r) = iter.next() {
                    let db_key = db.make_db_key(&r.key);
                    if db.is_expired(&db_key.p_key, &r.value) {
                        db.delete_update(&db_key.p_key)?;
                        continue;
                    }

                    // query_results.push(value)
                    query_results.push(db_key.p_key.clone());

                    // count the model id
                    if let Some(value) = key_count.get_mut(&db_key.p_key) {
                        *value += 1;
                    } else {
                        key_count.insert(db_key.p_key, 1);
                    }
                }
            }

            let keys: Vec<Box<[u8]>> = query_results
                .into_iter()
                .filter(|id| key_count[id] == cond_len)
                .take(limit)
                .collect();
            get_by_keys(&db, &self.name, &keys)
        } else {
            get_all(&db, &self.name, limit)
        };

        Ok(ret)
    }

    fn create(&self, model: &T) -> ActResult<bool> {
        trace!("local::{}.create({})", self.name, model.id());
        let db = self.db.read().unwrap();
        let data = model.to_vec()?;
        let cf = db.cf_handle(&self.name)?;
        db.put_cf(&cf, model.id().as_bytes(), data)?;

        for key in &T::keys() {
            if key == "id" {
                continue;
            }

            let cf = db.cf_idx_handle(&self.name, key)?;
            let value = model.get(key)?;
            let key = db.idx_key(key, &value, model.id().as_bytes());
            let seq = db.latest_sequence_number().to_le_bytes().to_vec();
            db.put_cf(&cf, key, seq)?;
        }

        Ok(true)
    }
    fn update(&self, model: &T) -> ActResult<bool> {
        trace!("local::{}.update({})", self.name, model.id());
        let db = self.db.read().unwrap();
        let data = model.to_vec()?;
        let cf = db.cf_handle(&self.name)?;

        let id = model.id().as_bytes();
        db.put_cf(&cf, id, data)?;
        db.update_change(id)?;
        for key in &T::keys() {
            if key == "id" {
                continue;
            }
            let cf = db.cf_handle(&self.name).unwrap();
            let value = model.get(key)?;
            let key = db.idx_key(key, &value, model.id().as_bytes());
            let seq = db.latest_sequence_number().to_le_bytes().to_vec();
            db.put_cf(&cf, key, seq)?;
        }

        Ok(true)
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        trace!("local::{}.delete({})", self.name, id);
        let db = self.db.read().unwrap();
        let cf = db.cf_handle(&self.name)?;
        let item = self.find(id)?;

        let id = id.as_bytes();
        db.delete_cf(&cf, id)?;

        // ignore error, because the data maybe not be changed.
        let _ = db.delete_update(id);
        for key in &T::keys() {
            if key == "id" {
                continue;
            }
            let cf = db.cf_idx_handle(&self.name, key)?;
            let db_value = item.get(key)?;
            let idx = db.idx_key(key, &db_value, item.id().as_bytes());
            db.delete_cf(&cf, &idx)?;
        }

        Ok(true)
    }
}

impl<T> DbSet for Collect<T>
where
    T: DbModel + Send + Sync,
{
    type Item = T;
    fn exists(&self, id: &str) -> ActResult<bool> {
        self.exists(id)
    }

    fn find(&self, id: &str) -> ActResult<T> {
        self.find(id)
    }

    fn query(&self, query: &Query) -> ActResult<Vec<T>> {
        self.query(query)
    }

    fn create(&self, model: &T) -> ActResult<bool> {
        self.create(model)
    }

    fn update(&self, model: &T) -> ActResult<bool> {
        self.update(model)
    }

    fn delete(&self, id: &str) -> ActResult<bool> {
        self.delete(id)
    }
}
