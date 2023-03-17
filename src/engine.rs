use crate::{
    adapter::{self, Adapter},
    debug,
    executor::Executor,
    manager::Manager,
    model::Workflow,
    options::Options,
    plugin::{self, ActPlugin},
    sch::{ActionOptions, Event, Scheduler},
    utils, ActError, ActModule, ActResult, Emitter, Vars,
};
use rhai::{EvalAltResult, Identifier, RegisterNativeFunction, Variant};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, Registry};

static IS_GLOBAL_LOG: Mutex<bool> = Mutex::new(false);

/// Workflow Engine
///
/// ## Example:
/// a example to caculate the result from 1 to given input value
///
///```rust
/// use acts::{Engine, State, Workflow, Vars};
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
///     let executor = engine.executor();
///     executor.start(&workflow);
///
///     let e = engine.clone();
///     engine.emitter().on_complete(move |w: &State<Workflow>| {
///         println!("{:?}", w.outputs());
///         e.close();
///     });
///     engine.start().await;
/// }
/// ```
#[derive(Clone)]
pub struct Engine {
    scher: Arc<Scheduler>,
    adapter: Arc<Adapter>,
    emitter: Arc<Emitter>,
    executor: Arc<Executor>,
    mgr: Arc<Manager>,
    is_closed: Arc<Mutex<bool>>,
}

impl Engine {
    pub fn new() -> Self {
        let config = utils::default_config();
        Engine::new_with(&config)
    }

    pub fn new_with(config: &Options) -> Self {
        let mut v = IS_GLOBAL_LOG.lock().unwrap();
        if *v == false {
            Registry::default().with(fmt::layer()).init();
            *v = true;
        }

        let scher = Arc::new(Scheduler::new_with(config));

        let engine = Engine {
            scher: scher.clone(),
            adapter: Arc::new(Adapter::new()),
            executor: Arc::new(Executor::new(&scher)),
            emitter: Arc::new(Emitter::new(&scher)),
            mgr: Arc::new(Manager::new()),

            is_closed: Arc::new(Mutex::new(false)),
        };

        engine
    }

    pub fn adapter(&self) -> Arc<Adapter> {
        self.adapter.clone()
    }

    /// engine executor
    pub fn executor(&self) -> Arc<Executor> {
        self.executor.clone()
    }

    /// event emitter
    pub fn emitter(&self) -> Arc<Emitter> {
        self.emitter.clone()
    }

    /// engine manager
    pub fn mgr(&self) -> Arc<Manager> {
        self.mgr.clone()
    }

    /// start engine
    ///
    /// ## Example
    /// ```rust
    /// use acts::{Engine, Workflow, Vars};
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
        scher.event_loop().await;
    }

    /// close engine
    ///
    /// ## Example
    ///
    /// ```rust
    /// use acts::{Engine, Workflow, Vars};
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

    pub(crate) fn scher(&self) -> Arc<Scheduler> {
        self.scher.clone()
    }

    async fn init(&self) {
        plugin::init(self).await;
        adapter::init(self).await;

        self.scher.init(self).await;
    }
}
