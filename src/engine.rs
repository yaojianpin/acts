use crate::{
    adapter::{self, Adapter},
    executor::Executor,
    manager::Manager,
    options::Options,
    plugin::{self},
    sch::Scheduler,
    store::Store,
    utils, Emitter,
};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Sender;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, Registry};

static IS_GLOBAL_LOG: Mutex<bool> = Mutex::new(false);

/// Workflow Engine
///
/// ## Example:
/// a example to caculate the result from 1 to given input value
///
///```rust
/// use acts::{ActionOptions, Engine, State, Workflow, Vars};
///
/// #[tokio::main]
/// async fn main() {
///     let engine = Engine::new();
///     engine.start();
///
///     let model = include_str!("../examples/simple/model.yml");
///     let mut workflow = Workflow::from_str(model).unwrap();
///
///     let mut vars = Vars::new();
///     vars.insert("input".into(), 3.into());
///     workflow.set_env(vars);
///     
///     engine.emitter().on_complete(move |w: &State<Workflow>| {
///         println!("{:?}", w.outputs());
///     });
///
///     let executor = engine.executor();
///     executor.deploy(&workflow).expect("fail to deploy workflow");
///     executor.start(
///        &workflow.id,
///        crate::ActionOptions {
///            biz_id: Some("w1".to_string()),
///            ..Default::default()
///        },
///    );
/// }
/// ```
#[derive(Clone)]
pub struct Engine {
    scher: Arc<Scheduler>,
    adapter: Arc<Adapter>,
    emitter: Arc<Emitter>,
    executor: Arc<Executor>,
    mgr: Arc<Manager>,
    store: Arc<Store>,
    signal: Arc<Mutex<Option<Sender<i32>>>>,
    is_closed: Arc<Mutex<bool>>,
}

impl Engine {
    pub fn new() -> Self {
        let config = utils::default_config();
        Engine::new_with(&config)
    }

    fn new_with(config: &Options) -> Self {
        let mut v = IS_GLOBAL_LOG.lock().unwrap();
        if *v == false {
            Registry::default().with(fmt::layer()).init();
            *v = true;
        }

        let scher = Arc::new(Scheduler::new_with(config));
        let store = Arc::new(Store::new());
        let engine = Engine {
            scher: scher.clone(),
            adapter: Arc::new(Adapter::new()),
            executor: Arc::new(Executor::new(&scher, &store)),
            emitter: Arc::new(Emitter::new(&scher)),
            mgr: Arc::new(Manager::new()),
            store: store.clone(),

            signal: Arc::new(Mutex::new(None)),
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
    ///     engine.start();
    /// }
    /// ```
    pub fn start(&self) {
        self.init();
        let scher = self.scher();
        tokio::spawn(async move { scher.event_loop().await });
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
    ///     engine.close();
    /// }
    /// ```
    pub fn close(&self) {
        let mut is_closed = self.is_closed.lock().unwrap();
        if *is_closed {
            return;
        }
        *is_closed = true;
        self.scher().close();

        if let Some(sig) = &*self.signal.lock().unwrap() {
            let s = sig.clone();
            tokio::spawn(async move { s.send(1).await });
        }
    }

    pub async fn r#loop(&self) {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        *self.signal.lock().unwrap() = Some(tx);
        rx.recv().await;
    }

    pub fn is_closed(self) -> bool {
        self.is_closed.lock().unwrap().clone()
    }

    pub(crate) fn scher(&self) -> Arc<Scheduler> {
        self.scher.clone()
    }

    /// engine store
    pub(crate) fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    fn init(&self) {
        plugin::init(self);
        adapter::init(self);

        self.scher.init(self);
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("is_closed", &self.is_closed)
            .finish()
    }
}
