use super::Enviroment;
use crate::{sch::Context, utils::consts, ActError, Result, ShareLock, Vars};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tracing::debug;

#[derive(Default, Clone)]
pub struct RefEnv {
    env: Arc<Enviroment>,
    scope: ShareLock<rhai::Scope<'static>>,
    id: String,
}

impl RefEnv {
    pub fn new(env: &Arc<Enviroment>, id: &str) -> Self {
        let scope = rhai::Scope::new();
        let mut vars = env.vars.borrow_mut();
        if !vars.contains_key(id) {
            vars.set(id, Vars::new());
        }
        let cell = Self {
            env: env.clone(),
            scope: Arc::new(RwLock::new(scope)),
            id: id.to_string(),
        };
        cell
    }

    pub fn set_parent(&self, parent_id: &str) {
        self.set(consts::ENV_PARENT_TASK_ID, parent_id);
    }

    pub fn bind_context(&self, ctx: &Context) {
        self.scope.write().unwrap().set_or_push("env", ctx.clone());
    }

    pub fn data(&self) -> Vars {
        if let Some(entry) = self.env.get::<Vars>(&self.id) {
            return entry;
        }

        Vars::new()
    }

    pub fn run(&self, script: &str) -> Result<bool> {
        let scr = self.env.scr.lock().unwrap();
        let mut scope = self.scope.write().unwrap();
        match scr.run_with_scope(&mut scope, script) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn eval<T: rhai::Variant + Clone>(&self, expr: &str) -> Result<T> {
        let scr = self.env.scr.lock().unwrap();
        let mut scope = self.scope.write().unwrap();
        match scr.eval_with_scope::<T>(&mut scope, expr) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::Script(format!("{}", err))),
        }
    }

    pub fn append(&self, vars: &Vars) {
        if let Some(entry) = self.env.vars.borrow_mut().get_mut(&self.id) {
            if let Some(entry) = entry.as_object_mut() {
                for (name, v) in vars {
                    entry
                        .entry(name.to_string())
                        .and_modify(|i| *i = v.clone())
                        .or_insert(v.clone());
                }
            }
        }
    }

    pub fn contains_key(&self, name: &str) -> bool {
        if let Some(ref entry) = self.env.get::<Vars>(&self.id) {
            return entry.contains_key(name);
        }
        false
    }

    /// get the env tree value
    pub fn get_env<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        debug!("get_env: task={} name={}", self.id, name);
        let data = self.data();
        if let Some(value) = data.get(name) {
            return Some(value);
        }

        let mut parent_id = data.get::<String>(consts::ENV_PARENT_TASK_ID);
        while let Some(ref id) = parent_id {
            if let Some(vars) = self.env.data(id) {
                if let Some(value) = vars.get::<T>(name) {
                    return Some(value);
                }
                parent_id = vars.get::<String>(consts::ENV_PARENT_TASK_ID);
            } else {
                parent_id = None;
            }
        }

        None
    }

    /// set the env tree vars
    pub fn set_env(self: &Arc<Self>, vars: &Vars) {
        debug!("set_env: task={} vars={}", self.id, vars);
        let mut refs = Vec::new();
        let mut data = self.env.data(&self.id).unwrap();
        while let Some(ref id) = data.get::<String>(consts::ENV_PARENT_TASK_ID) {
            refs.push(id.clone());
            data = self.env.data(id).unwrap();
        }

        let mut locals = Vars::new();
        for (name, value) in vars {
            let mut is_shared_var = false;
            for id in refs.iter().rev() {
                if self.env.update_data(id, name, value) {
                    is_shared_var = true;
                    break;
                }
            }

            if !is_shared_var {
                locals.set(name, value);
            }
        }
        self.env.set_data(&self.id, &locals);
    }

    pub fn get<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        self.data().get(name)
    }

    pub fn set<T>(&self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        self.env.set_data(&self.id, &Vars::new().with(name, value));
    }

    #[allow(unused)]
    pub fn remove(&self, name: &str) {
        if let Some(entry) = self.env.vars.borrow_mut().get_mut(&self.id) {
            if let Some(entry) = entry.as_object_mut() {
                if entry.contains_key(name) {
                    entry.remove(name).unwrap();
                }
            }
        }
    }
}
