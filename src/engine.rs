use crate::{
    adapter::{self, Adapter},
    debug,
    model::Workflow,
    options::Options,
    plugin::{self, ActPlugin},
    sch::{Event, Scheduler, UserData},
    utils, ActError, ActModule, ActResult, Vars,
};
use rhai::{EvalAltResult, Identifier, RegisterNativeFunction, Variant};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
/// Workflow Engine
///
/// ## Example:
/// a example to caculate the result from 1 to given input value
///
///```rust
/// use yao::{Engine, Workflow, Vars};
///
/// #[tokio::main]
/// async fn main() {
///     let engine = Engine::new();
///
///     let model = include_str!("../examples/simple/model.yml");
///     let mut workflow = Workflow::from_str(model).unwrap();
///
///     let mut vars = Vars::new();
///     vars.insert("input".into(), 3.into());
///     workflow.set_env(vars);
///
///     engine.push(&workflow);
///
///     let e = engine.clone();
///     engine.on_workflow_complete(move |w: &Workflow| {
///         println!("{:?}", w.outputs());
///         e.close();
///     });
///     engine.start().await;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Engine {
    action: Arc<Mutex<ActModule>>,
    modules: Arc<Mutex<HashMap<String, ActModule>>>,
    scher: Arc<Scheduler>,
    adapter: Arc<Adapter>,
    evts: Arc<Mutex<Vec<Event>>>,
    is_closed: Arc<Mutex<bool>>,
    pub(crate) plugins: Arc<Mutex<Vec<Box<dyn ActPlugin>>>>,
}

impl Engine {
    pub fn new() -> Self {
        let config = utils::default_config();
        Engine::new_with(&config)
    }

    pub fn new_with(config: &Options) -> Self {
        let scher = Arc::new(Scheduler::new_with(config));

        let engine = Engine {
            plugins: Arc::new(Mutex::new(Vec::new())),
            action: Arc::new(Mutex::new(ActModule::new())),
            modules: Arc::new(Mutex::new(HashMap::new())),
            evts: Arc::new(Mutex::new(Vec::new())),
            scher: scher,
            adapter: Arc::new(Adapter::new()),
            is_closed: Arc::new(Mutex::new(false)),
        };

        engine
    }

    pub fn adapter(&self) -> Arc<Adapter> {
        self.adapter.clone()
    }

    pub fn push(&self, workflow: &Workflow) {
        self.scher().push(workflow);
    }

    pub fn post_message(&self, id: &str, action: &str, user: &str, vars: Vars) -> ActResult<()> {
        debug!("post_message:{} action={} user={}", id, action, user);

        let scher = self.scher();
        let message = scher.message(id);
        match message {
            Some(mut message) => {
                message.data = Some(UserData {
                    action: action.to_string(),
                    user: user.to_string(),
                    vars,
                });
                scher.sched_message(&message);
            }
            None => return Err(ActError::MessageNotFoundError(id.to_string())),
        }

        Ok(())
    }

    /// start engine
    ///
    /// ## Example
    /// ```rust
    /// use yao::{Engine, Workflow, Vars};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     let e = engine.clone();
    ///     tokio::spawn(async move {
    ///         tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    ///         e.close();
    ///     });
    ///     engine.start().await;
    /// }
    /// ```
    pub async fn start(&self) {
        self.init().await;
        let scher = self.scher();
        loop {
            let ret = scher.next().await;
            if !ret {
                break;
            }
        }
    }

    /// register module
    ///
    /// ## Example
    /// ```rust
    /// #[tokio::test]
    /// async fn engine_register_module() {
    ///     let engine = Engine::new();
    ///     let mut module = Module::new();
    ///     combine_with_exported_module!(&mut module, "role", test_module);
    ///     engine.register_module("test", &module);
    ///     assert!(engine.modules().contains_key("test"));
    /// }
    /// ```
    pub fn register_module(&self, name: &str, module: &ActModule) {
        self.modules
            .lock()
            .unwrap()
            .insert(name.to_string(), module.clone());
    }

    /// register act function
    ///
    /// ## Example
    ///
    /// ```rust
    /// #[tokio::test]
    /// async fn engine_register_module() {
    ///     let mut engine = Engine::new();
    ///     let add = |a: i64, b: i64| Ok(a + b);
    ///     engine.register_action("add", add);
    /// }
    /// ```
    pub fn register_action<ARGS, N, T, F, S>(&mut self, name: N, func: F) -> u64
    where
        N: AsRef<str> + Into<Identifier>,
        T: Variant + Clone,
        F: RegisterNativeFunction<ARGS, T, std::result::Result<S, Box<EvalAltResult>>>,
    {
        self.action.lock().unwrap().set_native_fn(name, func)
    }

    /// close engine
    ///
    /// ## Example
    ///
    /// ```rust
    /// use yao::{Engine, Workflow, Vars};
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     engine.close()
    /// }
    /// ```
    pub fn close(&self) {
        *self.is_closed.lock().unwrap() = true;
        self.scher().close();
    }

    pub fn is_closed(self) -> bool {
        self.is_closed.lock().unwrap().clone()
    }

    pub(crate) fn modules(&self) -> HashMap<String, ActModule> {
        self.modules.lock().unwrap().clone()
    }

    pub(crate) fn action(&self) -> ActModule {
        self.action.lock().unwrap().clone()
    }

    pub(crate) fn evts(&self) -> Vec<Event> {
        self.evts.lock().unwrap().clone()
    }

    pub(crate) fn scher(&self) -> Arc<Scheduler> {
        self.scher.clone()
    }

    pub(crate) fn register_event(&self, evt: &Event) {
        self.evts.lock().unwrap().push(evt.clone());
    }

    async fn init(&self) {
        plugin::init(self).await;
        adapter::init(self).await;
        self.scher.init(self).await;
    }
}
