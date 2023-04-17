use crate::{env::Enviroment, sch::Context, ActError, ActResult, ActValue, ShareLock, Vars};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

#[derive(Default, Clone)]
pub struct VirtualMachine {
    env: Arc<Enviroment>,
    pub(crate) scope: ShareLock<rhai::Scope<'static>>,
    pub(crate) vars: ShareLock<Vars>,
}

impl VirtualMachine {
    pub fn new(env: &Enviroment) -> Self {
        let scope = rhai::Scope::new();

        let vm = Self {
            env: Arc::new(env.clone()),
            scope: Arc::new(RwLock::new(scope)),
            vars: Arc::new(RwLock::new(HashMap::new())),
        };

        vm.scope.write().unwrap().set_or_push("env", vm.clone());

        vm
    }

    pub fn init(&self, _ctx: &Context) {}

    pub fn vars(&self) -> Vars {
        self.vars.read().unwrap().clone()
    }

    pub fn set_scope_var<T: Send + Sync + Clone + 'static>(&self, name: &str, v: &T) {
        self.scope.write().unwrap().set_or_push(name, v.clone());
    }

    pub fn append(&self, vars: Vars) {
        self.vars.write().unwrap().extend(vars);
    }

    pub fn run(&self, script: &str) -> ActResult<bool> {
        match self.env.run(script, self) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::ScriptError(format!("{}", err))),
        }
    }

    pub fn eval<T: rhai::Variant + Clone>(&self, expr: &str) -> ActResult<T> {
        match self.env.eval::<T>(expr, self) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::ScriptError(format!("{}", err))),
        }
    }

    pub fn get(&self, name: &str) -> Option<ActValue> {
        let vars = self.vars.read().unwrap();
        match vars.get(name) {
            Some(v) => Some(v.clone()),
            None => None,
        }
    }

    pub fn set(&self, name: &str, value: ActValue) {
        let mut vars = self.vars.write().unwrap();
        vars.entry(name.to_string())
            .and_modify(|i| *i = value.clone())
            .or_insert(value);
    }
}
