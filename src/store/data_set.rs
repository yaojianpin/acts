use crate::ActResult;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Query {
    limit: usize,
    conds: HashMap<String, String>,
}

impl Query {
    pub fn new() -> Self {
        Query {
            limit: 0,
            conds: HashMap::new(),
        }
    }
    pub fn push(mut self, key: &str, value: &str) -> Self {
        self.conds.insert(key.to_string(), value.to_string());

        self
    }

    pub fn sql(&self) -> String {
        let mut ret = String::new();

        let cond_len = self.conds.len();
        if cond_len > 0 {
            ret.push_str(" where ");
            for (index, (key, value)) in self.conds.iter().enumerate() {
                ret.push_str(key);
                ret.push_str("=");
                ret.push_str(&format!("'{}'", value));

                if index != cond_len - 1 {
                    ret.push_str(" and ");
                }
            }
        }

        if self.limit > 0 {
            ret.push_str(&format!(" limit {}", self.limit));
        }

        ret
    }

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

pub trait DataSet<T>: Send + Sync {
    fn exists(&self, id: &str) -> bool;
    fn find(&self, id: &str) -> ActResult<T>;
    fn query(&self, query: &Query) -> ActResult<Vec<T>>;
    fn create(&self, data: &T) -> ActResult<bool>;
    fn update(&self, data: &T) -> ActResult<bool>;
    fn delete(&self, id: &str) -> ActResult<bool>;
}
