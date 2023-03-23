use crate::{
    debug,
    store::{DataSet, Message, Model, Proc, Task},
    ActError, ActResult, Context, Engine, ShareLock, Step,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

mod rule;

#[cfg(test)]
mod tests;

pub fn init(engine: &Engine) {
    debug!("adapter::init");

    // register inner some rule
    engine
        .adapter()
        .register_some_rule("rate", rule::Rate::default());
}

pub trait OrdRule: Send + Sync {
    fn ord(&self, users: &Vec<String>) -> ActResult<Vec<String>>;
}

pub trait SomeRule: Send + Sync {
    fn some(&self, step: &Step, ctx: &Context) -> ActResult<bool>;
}

/// Rule adapter trait
///
/// # Example
/// ```no_run
/// use acts::{RuleAdapter, ActResult, Context, Step};
/// struct TestAdapter;
/// impl RuleAdapter for TestAdapter {
///     fn ord(&self, _name: &str, acts: &Vec<String>) -> ActResult<Vec<String>> {
///         let mut ret = acts.clone();
///         ret.reverse();
///         Ok(ret)
///     }
///     fn some(
///         &self,
///         _name: &str,
///         _step: &Step,
///         _ctx: &Context,
///     ) -> ActResult<bool> {
///         Ok(true)
///     }
/// }
/// ```
pub trait RuleAdapter: Send + Sync {
    fn ord(&self, name: &str, acts: &Vec<String>) -> ActResult<Vec<String>>;
    fn some(&self, name: &str, step: &Step, ctx: &Context) -> ActResult<bool>;
}

/// Org adapter trait
///
/// # Example
/// ```no_run
/// use acts::{OrgAdapter};
/// struct TestAdapter;
/// impl OrgAdapter for TestAdapter {
///     fn dept(&self, _name: &str) -> Vec<String> {
///         vec!["u1".to_string(), "u2".to_string()]
///     }
///     fn unit(&self, _name: &str) -> Vec<String> {
///         vec![
///             "u1".to_string(),
///             "u2".to_string(),
///             "u3".to_string(),
///             "u4".to_string(),
///         ]
///     }
///     fn relate(&self, _id_type: &str, _id: &str, _relation: &str) -> Vec<String> {
///         vec!["p1".to_string()]
///     }
/// }
/// ```
pub trait OrgAdapter: Send + Sync {
    fn dept(&self, name: &str) -> Vec<String>;
    fn unit(&self, name: &str) -> Vec<String>;

    /// Get the users according to the relation
    ///
    /// { id_type } the id type, maybe user, depart, or unit, it can be judged by implementation
    /// { id } related id
    /// { relateion } a relation string, like `d.d.owner`,  
    ///     d.d is current id's deparetment's parent deparentment, `owner` means the position
    ///     finally, it will return a users list
    fn relate(&self, id_type: &str, id: &str, relation: &str) -> Vec<String>;
}

/// Role adapter trait
///
/// # Example
/// ```no_run
/// use acts::RoleAdapter;
/// struct TestAdapter;
/// impl RoleAdapter for TestAdapter {
///     fn role(&self, _name: &str) -> Vec<String> {
///         vec!["a1".to_string()]
///     }
/// }
/// ```
pub trait RoleAdapter: Send + Sync {
    fn role(&self, name: &str) -> Vec<String>;
}

/// Store adapter trait
/// Used to implement custom storage
///
/// # Example
/// ```no_run
/// use acts::{store::{Model, Proc, Task, Message, DataSet}, StoreAdapter};
/// use std::sync::Arc;
/// struct TestStore;
/// impl StoreAdapter for TestStore {
///
///     fn models(&self) -> Arc<dyn DataSet<Model>> {
///         todo!()
///     }
///     fn procs(&self) -> Arc<dyn DataSet<Proc>> {
///         todo!()
///     }
///     fn tasks(&self) -> Arc<dyn DataSet<Task>> {
///         todo!()
///     }
///     fn messages(&self) -> Arc<dyn DataSet<Message>> {
///         todo!()
///     }
///     fn init(&self) {}
///     fn flush(&self) {}
/// }
/// ```
pub trait StoreAdapter: Send + Sync {
    fn init(&self);

    fn models(&self) -> Arc<dyn DataSet<Model>>;
    fn procs(&self) -> Arc<dyn DataSet<Proc>>;
    fn tasks(&self) -> Arc<dyn DataSet<Task>>;
    fn messages(&self) -> Arc<dyn DataSet<Message>>;

    fn flush(&self);
}

#[derive(Clone)]
pub struct Adapter {
    role: ShareLock<Option<Arc<dyn RoleAdapter>>>,
    org: ShareLock<Option<Arc<dyn OrgAdapter>>>,
    store: ShareLock<Option<Arc<dyn StoreAdapter>>>,
    rule: ShareLock<Option<Arc<dyn RuleAdapter>>>,
    ords: ShareLock<HashMap<String, Box<dyn OrdRule>>>,
    somes: ShareLock<HashMap<String, Box<dyn SomeRule>>>,
}

impl Adapter {
    pub fn new() -> Self {
        Self {
            role: Arc::new(RwLock::new(None)),
            org: Arc::new(RwLock::new(None)),
            store: Arc::new(RwLock::new(None)),
            rule: Arc::new(RwLock::new(None)),
            ords: Arc::new(RwLock::new(HashMap::new())),
            somes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set_role_adapter<ROLE: RoleAdapter + 'static>(&self, _name: &str, role: ROLE) {
        debug!("set_role_adapter: {}", _name);
        *self.role.write().unwrap() = Some(Arc::new(role));
    }

    pub fn set_org_adapter<ORG: OrgAdapter + 'static>(&self, _name: &str, org: ORG) {
        debug!("set_org_adapter: {}", _name);
        *self.org.write().unwrap() = Some(Arc::new(org));
    }
    pub fn set_rule_adapter<RULE: RuleAdapter + 'static>(&self, _name: &str, rule: RULE) {
        debug!("set_rule_adapter: {}", _name);
        *self.rule.write().unwrap() = Some(Arc::new(rule));
    }
    pub fn set_store_adapter<STORE: StoreAdapter + 'static>(&self, _name: &str, store: STORE) {
        debug!("set_store_adapter: {}", _name);
        *self.store.write().unwrap() = Some(Arc::new(store));
    }

    pub fn store(&self) -> Option<Arc<dyn StoreAdapter>> {
        self.store.read().unwrap().clone()
    }

    pub fn register_ord_rule<T>(&self, name: &str, rule: T)
    where
        T: OrdRule + 'static,
    {
        debug!("register_ord_rule: {}", name);
        let mut rules = self.ords.write().unwrap();
        rules.insert(name.to_string(), Box::new(rule));
    }

    pub fn register_some_rule<T>(&self, name: &str, rule: T)
    where
        T: SomeRule + 'static,
    {
        debug!("register_some_rule: {}", name);
        let mut rules = self.somes.write().unwrap();
        rules.insert(name.to_string(), Box::new(rule));
    }
}

impl RuleAdapter for Adapter {
    fn ord(&self, name: &str, acts: &Vec<String>) -> ActResult<Vec<String>> {
        let rules = self.ords.read().unwrap();
        match rules.get(name) {
            Some(rule) => rule.ord(acts),
            None => {
                let rule = &*self.rule.read().unwrap();
                match rule {
                    Some(adapter) => adapter.ord(name, acts),
                    None => Err(ActError::AdapterError(format!("ord rule error ({})", name))),
                }
            }
        }
    }

    fn some(&self, name: &str, step: &Step, ctx: &Context) -> ActResult<bool> {
        let rules = self.somes.read().unwrap();
        match rules.get(name) {
            Some(rule) => rule.some(step, ctx),
            None => {
                let rule = &*self.rule.read().unwrap();
                match rule {
                    Some(adapter) => adapter.some(name, step, ctx),
                    None => Err(ActError::AdapterError(format!("ord rule error ({})", name))),
                }
            }
        }
    }
}

impl OrgAdapter for Adapter {
    fn dept(&self, name: &str) -> Vec<String> {
        let mut ret = Vec::new();
        if let Some(adapter) = &*self.org.read().unwrap() {
            ret = adapter.dept(name);
        }

        ret
    }

    fn unit(&self, name: &str) -> Vec<String> {
        let mut ret = Vec::new();
        if let Some(adapter) = &*self.org.read().unwrap() {
            ret = adapter.unit(name);
        }

        ret
    }

    fn relate(&self, t: &str, id: &str, r: &str) -> Vec<String> {
        let mut ret = Vec::new();
        if let Some(adapter) = &*self.org.read().unwrap() {
            ret = adapter.relate(t, id, r);
        }

        ret
    }
}

impl RoleAdapter for Adapter {
    fn role(&self, name: &str) -> Vec<String> {
        let mut ret = Vec::new();
        if let Some(adapter) = &*self.role.read().unwrap() {
            ret = adapter.role(name);
        }

        ret
    }
}

// impl ActStore for Adapter {
//     fn procs(&self) -> Arc<dyn Set<Proc>> {
//         self.store.read().unwrap().procs()
//     }

//     fn tasks(&self) -> Arc<dyn Set<Task>> {
//         self.store.read().unwrap().tasks()
//     }

//     fn messages(&self) -> Arc<dyn Set<Message>> {
//         self.store.read().unwrap().messages()
//     }
// }
