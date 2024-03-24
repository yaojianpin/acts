use super::utils::SEP;

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
}
