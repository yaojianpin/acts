use std::{collections::HashSet, slice::IterMut};

#[derive(Debug, Clone)]
pub struct Query {
    offset: usize,
    limit: usize,
    conds: Vec<Cond>,
}

#[derive(Debug, Clone)]
pub enum CondType {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct Cond {
    pub r#type: CondType,
    pub conds: Vec<Expr>,
    pub result: HashSet<Box<[u8]>>,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub key: String,
    pub value: String,
    pub result: HashSet<Box<[u8]>>,
}

impl Cond {
    pub fn or() -> Self {
        Self {
            r#type: CondType::Or,
            conds: Vec::new(),
            result: HashSet::new(),
        }
    }

    pub fn and() -> Self {
        Self {
            r#type: CondType::And,
            conds: Vec::new(),
            result: HashSet::new(),
        }
    }

    pub fn push(mut self, expr: Expr) -> Self {
        self.conds.push(expr);

        self
    }

    
}

impl Expr {
    pub fn eq(key: &str, value: &str) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
            result: HashSet::new(),
        }
    }
}

impl Query {
    pub fn new() -> Self {
        Query {
            offset: 0,
            limit: 100000, // default to a big number
            conds: Vec::new(),
        }
    }

    pub fn queries_mut(&mut self) -> IterMut<'_, Cond> {
        self.conds.iter_mut()
    }

    pub fn queries(&mut self) -> &Vec<Cond> {
        &self.conds
    }

    pub fn calc(&self) -> HashSet<Box<[u8]>> {
        let mut result = HashSet::new();
        for cond in self.conds.iter() {
            if result.len() == 0 {
                result = cond.result.clone();
            } else {
                result = result
                    .intersection(&cond.result)
                    .cloned()
                    .collect::<HashSet<_>>()
            }
        }
        result
    }

    pub fn push(mut self, cond: Cond) -> Self {
        self.conds.push(cond);

        self
    }

    pub fn set_offset(mut self, offset: usize) -> Self {
        self.offset = offset;

        self
    }

    pub fn set_limit(mut self, limit: usize) -> Self {
        self.limit = limit;

        self
    }

    pub fn limit(&self) -> usize {
        if self.limit == 0 {
            return 10000;
        }

        self.limit
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn is_cond(&self) -> bool {
        self.conds.len() > 0
    }
}
