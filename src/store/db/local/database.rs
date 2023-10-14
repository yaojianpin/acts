use crate::{
    store::db::{map_db_err, map_opt_err},
    Result,
};
use rocksdb::{
    AsColumnFamilyRef, BoundColumnFamily, /*ColumnFamily as SingleColumnFamily,*/ DBIterator,
    IteratorMode, Options, DB,
};
use std::sync::Arc;

pub const CF_UPDATE: &str = "__update";
pub const SEP: &str = r"\n";
pub type RocksDB = DB;
pub type ColumnFamily<'a> = Arc<BoundColumnFamily<'a>>;

pub struct Database {
    path: String,
    db: RocksDB,
    opts: Options,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").field("db", &self.db).finish()
    }
}

#[derive(Debug)]
pub struct DbKey {
    pub idx_key: Box<[u8]>,
    pub p_key: Box<[u8]>,
}

pub struct QueryIter<'a> {
    #[allow(unused)]
    db: &'a Database,
    iter: DBIterator<'a>,
}

pub struct QueryResult {
    pub key: Box<[u8]>,
    pub value: Box<[u8]>,
}

impl<'a> QueryIter<'a> {
    fn new(db: &'a Database, iter: DBIterator<'a>) -> Self {
        Self { db, iter }
    }
}

impl<'a> Iterator for QueryIter<'a> {
    type Item = QueryResult;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(Ok((key, value))) = self.iter.next() {
            return Some(QueryResult { key, value });
        }

        None
    }
}

impl Database {
    pub fn new(path: &str) -> Self {
        let mut opts = Options::default();
        opts.create_missing_column_families(false);
        opts.create_if_missing(true);
        opts.set_max_total_wal_size(1024 * 1024);

        let cfs = DB::list_cf(&opts, path).unwrap_or_else(|_| vec![]);
        let db = DB::open_cf(&opts, path, cfs.clone()).unwrap();
        if cfs.is_empty() {
            // init cf_update
            db.create_cf(CF_UPDATE, &Options::default()).unwrap();
        }
        Self {
            db,
            opts,
            path: path.to_string(),
        }
    }

    pub fn idx_key<'a>(&self, _attr: &str, key: &[u8], id: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(key.as_ref());
        bytes.extend_from_slice(SEP.as_ref());
        bytes.extend_from_slice(id.as_ref());

        bytes
    }

    pub fn cf_idx_handle(&self, name: &str, key: &str) -> Result<ColumnFamily> {
        let cf_name = format!("{name}:{key}");
        self.cf_handle(&cf_name)
    }

    pub fn init_cfs(&mut self, name: &str, keys: &[String]) {
        let cfs = DB::list_cf(&self.opts, &self.path).unwrap_or_else(|_| vec![]);
        let cf = format!("{name}");
        if !cfs.contains(&cf) {
            self.create_innter_cf(&cf);
        }

        for key in keys {
            let cf = format!("{name}:{key}");
            if cfs.contains(&cf) {
                continue;
            }
            self.create_innter_cf(&cf);
        }
    }

    pub fn cf_handle(&self, name: &str) -> Result<ColumnFamily> {
        self.cf_inner_handle(&name)
            .ok_or(map_opt_err(format!("cannot find Collect({name})")))
    }

    pub fn get_cf<K: AsRef<[u8]>>(
        &self,
        name: &str,
        cf: &impl AsColumnFamilyRef,
        key: K,
    ) -> Result<Vec<u8>> {
        let key_name = key.as_ref().to_vec();
        self.db
            .get_cf(cf, key)
            .map_err(map_db_err)?
            .ok_or(map_opt_err(format!(
                "cannot find data {}({})",
                name,
                String::from_utf8(key_name).unwrap()
            )))
    }

    pub fn put_cf<K, V>(&self, cf: &impl AsColumnFamilyRef, key: K, value: V) -> Result<bool>
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        self.db
            .put_cf(cf, key.as_ref(), value.as_ref())
            .map_err(map_db_err)?;

        Ok(true)
    }

    pub fn delete_cf<K: AsRef<[u8]>>(&self, cf: &impl AsColumnFamilyRef, key: K) -> Result<bool>
    where
        K: AsRef<[u8]>,
    {
        self.db.delete_cf(cf, key.as_ref()).map_err(map_db_err)?;
        Ok(true)
    }

    pub fn iterator_cf<'a: 'b, 'b>(
        &'a self,
        cf_handle: &impl AsColumnFamilyRef,
        mode: IteratorMode,
    ) -> QueryIter<'a> {
        let iter = self.db.iterator_cf(cf_handle, mode);
        QueryIter::new(&self, iter)
    }

    #[allow(unused)]
    pub fn prefix_iterator_cf<'a: 'b, 'b, P: AsRef<[u8]>>(
        &'a self,
        cf_handle: &impl AsColumnFamilyRef,
        prefix: P,
    ) -> QueryIter<'a> {
        let iter = self.db.prefix_iterator_cf(cf_handle, prefix);
        QueryIter::new(&self, iter)
    }

    pub fn update_change(&self, key: &[u8]) -> Result<bool> {
        let cf = self.cf_handle(CF_UPDATE)?;
        let seq = self.db.latest_sequence_number().to_le_bytes();
        self.put_cf(&cf, key, seq)?;
        Ok(true)
    }

    pub fn delete_update(&self, key: &[u8]) -> Result<bool> {
        let cf_update = self.cf_handle(CF_UPDATE)?;
        self.delete_cf(&cf_update, key.as_ref())?;

        Ok(true)
    }

    pub fn is_expired(&self, key: &[u8], seq: &[u8]) -> bool {
        if let Ok(cf) = self.cf_handle(CF_UPDATE) {
            if let Ok(Some(data)) = self.db.get_cf(&cf, key) {
                let size = std::mem::size_of::<u64>();
                let val = u64::from_le_bytes((&data[..size]).try_into().unwrap());
                let seq = u64::from_le_bytes((&seq[..size]).try_into().unwrap());

                return val < seq;
            }
        }

        false
    }

    pub fn latest_sequence_number(&self) -> u64 {
        self.db.latest_sequence_number()
    }

    pub fn make_db_key(&self, key: &[u8]) -> DbKey {
        let sep = SEP.as_bytes();
        let mut start = 0;
        let mut result = Vec::new();
        for (idx, win) in key.windows(sep.len()).enumerate() {
            if sep == win {
                result.push(&key[start..idx]);
                start = idx + sep.len();
            }
        }

        if start < key.len() {
            result.push(&key[start..key.len()]);
        }

        DbKey {
            idx_key: result[0].to_vec().into_boxed_slice(),
            p_key: result[1].to_vec().into_boxed_slice(),
        }
    }

    pub fn flush(&self) {
        let _ = self.db.flush();
    }

    fn cf_inner_handle(&self, name: &str) -> Option<ColumnFamily> {
        self.db.cf_handle(&name)
    }

    fn create_innter_cf(&mut self, name: &str) {
        let _ = self.db.create_cf(&name, &Options::default());
    }
}
