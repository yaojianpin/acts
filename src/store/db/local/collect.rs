use super::database::Database;
use crate::{
    store::{data::*, DbSet, Query},
    Result, ShareLock,
};
use acts_tag::Value;
use rocksdb::{Direction, IteratorMode};
use tracing::debug;

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
        if let Ok(data) = db.get_cf(model_name, &cf, it) {
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
    fn exists(&self, id: &str) -> Result<bool> {
        debug!("local::{}.exists({})", self.name, id);
        let db = self.db.read().unwrap();
        let cf = db.cf_handle(&self.name).unwrap();
        let ret = db.get_cf(&self.name, &cf, id.as_bytes());

        Ok(ret.is_ok())
    }

    fn find(&self, id: &str) -> Result<T> {
        debug!("local::{}.find({})", self.name, id);
        let db = self.db.read().unwrap();
        let cf = db.cf_handle(&self.name)?;
        let data = db.get_cf(&self.name, &cf, id.as_bytes())?;
        let model = T::from_slice(&data)?;
        Ok(model)
    }

    fn query(&self, q: &Query) -> Result<Vec<T>> {
        debug!("local::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        let mut limit = q.limit();
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }

        let ret = if q.is_cond() {
            let mut q = q.clone();
            for cond in q.queries_mut() {
                for expr in cond.conds.iter_mut() {
                    let cf = db.cf_idx_handle(&self.name, &expr.key)?;

                    let value = Value::from(&expr.value).unwrap();
                    let mut iter =
                        db.iterator_cf(&cf, IteratorMode::From(value.data(), Direction::Forward));
                    // let mut iter = db.prefix_iterator_cf(&cf, value.data());
                    while let Some(r) = iter.next() {
                        let db_key = db.make_db_key(&r.key);
                        if db_key.idx_key.as_ref() != value.data() {
                            break;
                        }
                        if db.is_expired(&db_key.p_key, &r.value) {
                            db.delete_update(&db_key.p_key)?;
                            continue;
                        }
                        expr.result.insert(db_key.p_key.clone());
                    }
                }
                cond.calc();
            }

            let keys = q.calc().into_iter().take(limit).collect::<Vec<_>>();
            get_by_keys(&db, &self.name, &keys)
        } else {
            get_all(&db, &self.name, limit)
        };

        Ok(ret)
    }

    fn create(&self, model: &T) -> Result<bool> {
        debug!("local::{}.create({})", self.name, model.id());
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
    fn update(&self, model: &T) -> Result<bool> {
        debug!("local::{}.update({})", self.name, model.id());
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
    fn delete(&self, id: &str) -> Result<bool> {
        debug!("local::{}.delete({})", self.name, id);
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
    fn exists(&self, id: &str) -> Result<bool> {
        self.exists(id)
    }

    fn find(&self, id: &str) -> Result<T> {
        self.find(id)
    }

    fn query(&self, query: &Query) -> Result<Vec<T>> {
        self.query(query)
    }

    fn create(&self, model: &T) -> Result<bool> {
        self.create(model)
    }

    fn update(&self, model: &T) -> Result<bool> {
        self.update(model)
    }

    fn delete(&self, id: &str) -> Result<bool> {
        self.delete(id)
    }
}
