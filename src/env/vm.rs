use crate::{env::Enviroment, sch::Context, ActError, ActResult, ActValue, ShareLock, Vars};
use std::sync::{Arc, RwLock};

#[derive(Default, Clone)]
pub struct VirtualMachine {
    env: Arc<Enviroment>,
    pub(crate) scope: ShareLock<rhai::Scope<'static>>,
    vars: ShareLock<Vars>,
}

impl VirtualMachine {
    pub fn new(env: &Enviroment) -> Self {
        let scope = rhai::Scope::new();

        let vm = Self {
            env: Arc::new(env.clone()),
            scope: Arc::new(RwLock::new(scope)),
            vars: Arc::new(RwLock::new(Vars::new())),
        };

        vm.scope.write().unwrap().set_or_push("env", vm.clone());

        vm
    }

    pub fn bind_context(&self, ctx: &Context) {
        self.scope.write().unwrap().set_or_push("act", ctx.clone());
    }

    pub fn vars(&self) -> Vars {
        self.vars.read().unwrap().clone()
    }

    pub fn set_scope_var<T: Send + Sync + Clone + 'static>(&self, name: &str, v: &T) {
        self.scope.write().unwrap().set_or_push(name, v.clone());
    }

    pub fn append(&self, vars: &Vars) {
        for (name, v) in vars {
            self.set(name, v.clone());
        }
    }

    pub fn output(&self, vars: &Vars) {
        self.env.append(vars);
    }

    pub fn run(&self, script: &str) -> ActResult<bool> {
        match self.env.run_vm(script, self) {
            Ok(..) => Ok(true),
            Err(err) => Err(ActError::Script(format!("{err} in {script}"))),
        }
    }

    pub fn eval<T: rhai::Variant + Clone>(&self, expr: &str) -> ActResult<T> {
        match self.env.eval_vm::<T>(expr, self) {
            Ok(ret) => Ok(ret),
            Err(err) => Err(ActError::Script(format!("{err} in {expr}"))),
        }
    }

    pub fn get(&self, name: &str) -> Option<ActValue> {
        // check the env global vars firstly
        self.env.get(name).or_else(|| {
            let vars = self.vars.read().unwrap();
            match vars.get(name) {
                Some(v) => Some(v.clone()),
                None => None,
            }
        })
    }

    pub fn set(&self, name: &str, value: ActValue) {
        if self.env.vars().contains_key(name) {
            return self.env.set(name, value);
        }

        let mut vars = self.vars.write().unwrap();
        vars.entry(name.to_string())
            .and_modify(|i| *i = value.clone())
            .or_insert(value);
    }

    pub fn remove(&self, name: &str) {
        if self.env.vars().contains_key(name) {
            return self.env.remove(name);
        }

        let mut vars = self.vars.write().unwrap();
        vars.remove(name);
    }
}
