use crate::{
    adapter::{self, Adapter},
    config::Config,
    env::Enviroment,
    export::{Emitter, Executor, Extender, Manager},
    plugin,
    sch::Scheduler,
    ActPlugin, Signal, StoreAdapter,
};

#[cfg(feature = "multi-thread")]
use once_cell::sync::OnceCell;
#[cfg(not(feature = "multi-thread"))]
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use tracing::info;

#[cfg(not(feature = "multi-thread"))]
thread_local! {
    static RUNTIME_LOCAL: RefCell<Option<Runtime>> = RefCell::new(None);
}

#[cfg(feature = "multi-thread")]
static RUNTIME: OnceCell<Arc<Runtime>> = OnceCell::new();

/// Workflow Engine
///
/// ## Example:
/// a example to caculate the result from 1 to given input value
///
///```rust,no_run
/// use acts::{Engine, Workflow, Vars};
///
/// #[tokio::main]
/// async fn main() {
///     let engine = Engine::new();
///
///     let model = include_str!("../examples/simple/model.yml");
///     let workflow = Workflow::from_yml(model).unwrap();
///     
///     engine.emitter().on_complete(|e| {
///         println!("{:?}", e.outputs());
///     });
///
///     engine.manager().deploy(&workflow).expect("fail to deploy workflow");
///
///     let mut vars = Vars::new();
///     vars.insert("input".into(), 3.into());
///     vars.insert("pid".into(), "test1".into());
///     engine.executor().start(
///        &workflow.id,
///        &vars);
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Engine {}

pub(crate) struct Runtime {
    config: Arc<Config>,
    scher: Arc<Scheduler>,
    env: Arc<Enviroment>,
    adapter: Arc<Adapter>,
    executor: Arc<Executor>,
    manager: Arc<Manager>,
    emitter: Arc<Emitter>,
    extender: Arc<Extender>,
    plugins: Arc<Mutex<Vec<Box<dyn ActPlugin>>>>,
}

impl Runtime {
    pub fn start(config: &Config, store: Option<Arc<dyn StoreAdapter>>) {
        #[cfg(feature = "multi-thread")]
        {
            let runtime = RUNTIME.get_or_init(|| Arc::new(Self::create(config, store)));
            runtime.event_loop();
        }

        #[cfg(not(feature = "multi-thread"))]
        {
            let runtime = Self::create(config, store);
            runtime.event_loop();
            RUNTIME_LOCAL.set(Some(runtime));
        }
    }

    pub fn with<T, F: Fn(&Runtime) -> T>(f: F) -> T {
        #[cfg(feature = "multi-thread")]
        {
            let runtime = RUNTIME.get().expect("fail to get current acts runtime");
            f(runtime)
        }

        #[cfg(not(feature = "multi-thread"))]
        {
            RUNTIME_LOCAL
                .with_borrow(|rt| rt.as_ref().map(|runtime| f(runtime)))
                .expect("fail to get current acts runtime")
        }
    }

    #[allow(unused)]
    pub fn scher(&self) -> &Arc<Scheduler> {
        &self.scher
    }
    #[allow(unused)]
    pub fn env(&self) -> &Arc<Enviroment> {
        &self.env
    }

    #[allow(unused)]
    pub fn adapter(&self) -> &Arc<Adapter> {
        &self.adapter
    }

    #[allow(unused)]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    #[allow(unused)]
    pub fn is_running(&self) -> bool {
        self.scher.is_closed() == false
    }

    fn event_loop(&self) {
        let scher = self.scher.clone();
        tokio::spawn(async move {
            scher.event_loop().await;
        });
    }

    fn create(config: &Config, store: Option<Arc<dyn StoreAdapter>>) -> Runtime {
        let scher = Scheduler::new_with(config);
        let plugins = Arc::new(Mutex::new(Vec::new()));
        let env = Arc::new(Enviroment::new());
        let extender = Arc::new(Extender::new(&env, &plugins));
        let executor = Arc::new(Executor::new(&scher));
        let manager = Arc::new(Manager::new(&scher));
        let emitter = Arc::new(Emitter::new(&scher));
        let adapter = Arc::new(Adapter::new());
        if let Some(store) = store {
            adapter.set_store(store);
        }

        Runtime {
            config: Arc::new(config.clone()),
            scher,
            env,
            adapter,
            executor,
            manager,
            emitter,
            extender,
            plugins,
        }
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::new_with_config(&Config::default(), None)
    }

    pub fn config(&self) -> Arc<Config> {
        Runtime::with(|rt| rt.config.clone())
    }

    /// engine executor
    pub fn executor(&self) -> Arc<Executor> {
        Runtime::with(|rt| rt.executor.clone())
    }

    /// event emitter
    pub fn emitter(&self) -> Arc<Emitter> {
        Runtime::with(|rt| rt.emitter.clone())
    }

    /// engine manager
    pub fn manager(&self) -> Arc<Manager> {
        Runtime::with(|rt| rt.manager.clone())
    }

    /// engine extender
    pub fn extender(&self) -> Arc<Extender> {
        Runtime::with(|rt| rt.extender.clone())
    }

    pub(crate) fn scher(&self) -> Arc<Scheduler> {
        Runtime::with(|rt| rt.scher.clone())
    }

    pub fn env(&self) -> Arc<Enviroment> {
        Runtime::with(|rt| rt.env.clone())
    }

    pub(crate) fn plugins(&self) -> Arc<Mutex<Vec<Box<dyn ActPlugin>>>> {
        Runtime::with(|rt| rt.plugins.clone())
    }

    /// close engine
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use acts::{Engine, Workflow, Vars};
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new();
    ///     engine.close();
    /// }
    /// ```
    pub fn close(self) {
        info!("close");
        self.scher().close();
    }

    pub fn signal<T: Clone>(&self, init: T) -> Signal<T> {
        Signal::new(init)
    }

    pub fn is_running(&self) -> bool {
        Runtime::with(|rt| rt.is_running())
    }

    fn init(&self) {
        info!("init");
        plugin::init(self);
        adapter::init(self);
        self.scher().init();
    }

    pub(crate) fn new_with_config(config: &Config, store: Option<Arc<dyn StoreAdapter>>) -> Self {
        info!("config: {:?}", config);
        Runtime::start(config, store);

        let engine = Self {};
        engine.init();
        engine
    }
}
