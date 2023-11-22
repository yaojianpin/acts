use acts_tag::Value;
use tracing::debug;

use crate::store::db::map_db_err;
use crate::Query;
use crate::{store::DbModel, ActError, DbSet, ShareLock};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct Collect<T> {
    name: String,
    db: ShareLock<BTreeMap<String, T>>,
}

impl<T> Collect<T> {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            db: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl<T> DbSet for Collect<T>
where
    T: DbModel + Send + Sync + Clone + Debug,
{
    type Item = T;

    fn exists(&self, id: &str) -> crate::Result<bool> {
        debug!("mem::{}.exists({:?})", self.name, id);
        Ok(self.db.read().unwrap().contains_key(id))
    }

    fn find(&self, id: &str) -> crate::Result<Self::Item> {
        debug!("mem::{}.find({:?})", self.name, id);
        self.db
            .read()
            .unwrap()
            .get(id)
            .map(|iter| iter.clone())
            .ok_or(ActError::Store(format!(
                "cannot find {} by '{}'",
                self.name, id
            )))
    }

    fn query(&self, q: &Query) -> crate::Result<Vec<Self::Item>> {
        debug!("mem::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        let mut limit = q.limit();
        if limit == 0 {
            // should be a big number to take
            limit = 10000;
        }
        if q.is_cond() {
            let mut q = q.clone();
            for cond in q.queries_mut() {
                for expr in cond.conds.iter_mut() {
                    for (k, v) in db.iter() {
                        let prop_value = v.get(&expr.key)?;
                        let cond_value = Value::from(&expr.value).map_err(map_db_err)?;
                        if prop_value == cond_value.data() {
                            expr.result.insert(k.as_bytes().to_vec().into_boxed_slice());
                        }
                    }
                }
                cond.calc();
            }

            let keys = q.calc().into_iter().take(limit).collect::<Vec<_>>();
            let ret = db
                .iter()
                .filter_map(|(k, v)| {
                    if keys.contains(&k.as_bytes().to_vec().into_boxed_slice()) {
                        return Some(v.clone());
                    }
                    None
                })
                .collect::<Vec<_>>();

            return Ok(ret);
        }
        Ok(db.values().take(limit).cloned().collect::<Vec<_>>())
    }

    fn create(&self, data: &Self::Item) -> crate::Result<bool> {
        debug!("mem::{}.create({:?})", self.name, data);
        self.db
            .write()
            .unwrap()
            .insert(data.id().to_string(), data.clone());
        Ok(true)
    }

    fn update(&self, data: &Self::Item) -> crate::Result<bool> {
        debug!("mem::{}.update({:?})", self.name, data);
        self.db
            .write()
            .unwrap()
            .entry(data.id().to_string())
            .and_modify(|iter| *iter = data.clone());
        Ok(true)
    }

    fn delete(&self, id: &str) -> crate::Result<bool> {
        debug!("mem::{}.delete({:?})", self.name, id);
        self.db.write().unwrap().remove(id);
        Ok(true)
    }
}
