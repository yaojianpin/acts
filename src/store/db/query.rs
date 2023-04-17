use super::utils::SEP;
use rocksdb::{ColumnFamily, DBIterator};

#[derive(Debug)]
pub struct DbQueryValue {
    pub key: Box<[u8]>,
    pub value: Box<[u8]>,
}

#[derive(Debug)]
pub struct DbKey {
    pub key_name: Box<[u8]>,
    pub key_value: Box<[u8]>,
    pub model_id: Box<[u8]>,
}

impl DbQueryValue {
    pub fn idx_key(&self) -> Box<[u8]> {
        let mut ret = Vec::new();

        ret.extend_from_slice(self.key.as_ref());
        ret.extend_from_slice(SEP.as_ref());
        ret.extend_from_slice(self.value.as_ref());

        ret.into_boxed_slice()
    }
}

#[derive(Debug)]
pub struct Query {
    limit: usize,
    conds: Vec<DbQueryValue>,
}

impl Query {
    pub fn new() -> Self {
        Query {
            limit: 0,
            conds: Vec::new(),
        }
    }
    pub fn push(mut self, key: Vec<u8>, value: Vec<u8>) -> Self {
        self.conds.push(DbQueryValue {
            key: key.into_boxed_slice(),
            value: value.into_boxed_slice(),
        });

        self
    }

    pub fn queries(&self) -> Vec<DbQueryValue> {
        self.conds
    }

    // pub fn sql(&self) -> String {
    //     let mut ret = String::new();

    //     let cond_len = self.conds.len();
    //     if cond_len > 0 {
    //         ret.push_str(" where ");
    //         for (index, (key, value)) in self.conds.iter().enumerate() {
    //             ret.push_str(key);
    //             ret.push_str("=");
    //             ret.push_str(&format!("'{}'", value));

    //             if index != cond_len - 1 {
    //                 ret.push_str(" and ");
    //             }
    //         }
    //     }

    //     if self.limit > 0 {
    //         ret.push_str(&format!(" limit {}", self.limit));
    //     }

    //     ret
    // }

    pub fn predicate<F: Fn(&str, &str) -> Vec<String>>(&self, f: F) -> Vec<String> {
        let mut ret = Vec::new();
        for (key, value) in &self.conds {
            let list = f(key, value);
            if ret.len() == 0 {
                ret = list;
            } else {
                ret = ret.into_iter().filter(|it| list.contains(it)).collect();
            }
        }

        if self.limit > 0 {
            ret = ret.into_iter().take(self.limit).collect();
        }
        ret
    }

    pub fn set_limit(mut self, limit: usize) -> Self {
        self.limit = limit;

        self
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn is_cond(&self) -> bool {
        self.conds.len() > 0
    }

    // fn convert(v: &ActValue) -> String {
    //     let ret: String = serde_yaml::to_string(v).unwrap();
    //     if v.is_string() {
    //         return format!("'{}'", v.as_str().unwrap());
    //     }

    //     ret
    // }
}

pub struct QueryIterator<'a> {
    db_iter: DBIterator<'a>,
    cf: &'a ColumnFamily,
}

impl<'a> QueryIterator<'a> {}
