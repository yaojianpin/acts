use crate::{
    adapter::{self, Adapter},
    export::{Emitter, Executor, Extender, Manager},
    options::Options,
    plugin,
    sch::Scheduler,
    store::Store,
};
use once_cell::sync::OnceCell;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Sender;
use tracing::info;

static STORE: OnceCell<Arc<Store>> = OnceCell::new();

/// Workflow Engine
///
/// ## Example:
/// a example to caculate the result from 1 to given input value
///
///```rust
/// use acts::{Engine, WorkflowState, Workflow, Vars};
///
/// #[tokio::main]
/// async fn main() {
///     let engine = Engine::new();
///     engine.start();
///
///     let model = include_str!("../examples/simple/model.yml");
///     let mut workflow = Workflow::from_str(model).unwrap();
///     
///     engine.emitter().on_complete(move |w: &WorkflowState| {
///         println!("{:?}", w.outputs());
///     });
///
///     engine.manager().deploy(&workflow).expect("fail to deploy workflow");
///
///     let mut vars = Vars::new();
///     vars.insert("input".into(), 3.into());
///     vars.insert("biz_id".into(), "w1".into());
///     engine.executor().start(
///        &workflow.id,
///        &vars);
/// }
/// ```
#[derive(Clone)]
pub struct Engine {
    scher: Arc<Scheduler>,
    adapter: Arc<Adapter>,
    executor: Arc<Executor>,
    manager: Arc<Manager>,
    emitter: Arc<Emitter>,
    extender: Arc<Extender>,
    store: Arc<Store>,
    signal: Arc<Mutex<Option<Sender<i32>>>>,
    is_closed: Arc<Mutex<bool>>,
}

impl Engine {
    pub fn new() -> Self {
        Self::new_with_options(&Options::default())
    }

    pub fn new_with_options(opt: &Options) -> Self {
        info!("options: {:?}", opt);
        let scher = Scheduler::new_with(opt);
        let store = STORE.get_or_init(|| Arc::new(Store::new_with_path(&opt.data_dir)));
        let engine = Engine {
            scher: scher.clone(),
            adapter: Arc::new(Adapter::new()),
            executor: Arc::new(Executor::new(&scher, &store)),
            manager: Arc::new(Manager::new(&scher, &store)),
            emitter: Arc::new(Emitter::new(&scher)),
            extender: Arc::new(Extender::new()),
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
    pub fn manager(&self) -> Arc<Manager> {
        self.manager.clone()
    }

    /// engine extender
    pub fn extender(&self) -> Arc<Extender> {
        self.extender.clone()
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
        info!("start");
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
        info!("close");
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

    pub async fn eloop(&self) {
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
        info!("init");
        plugin::init(self);
        adapter::init(self);
        self.store.init(self);
        self.scher.init(self);
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("store", &self.store().kind())
            .finish()
    }
}
