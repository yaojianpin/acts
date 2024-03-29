use crate::{
    sch::{queue::Signal, Scheduler},
    Engine, ShareLock,
};
use std::sync::{Arc, RwLock};
use tokio::{runtime::Handle, sync::mpsc, sync::Mutex};

#[derive(Clone)]
pub struct Queue {
    receiver: Arc<Mutex<mpsc::Receiver<Signal>>>,
    sender: Arc<mpsc::Sender<Signal>>,

    scher: ShareLock<Option<Arc<Scheduler>>>,
}

impl Queue {
    pub fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Signal>(100);

        let queue = Arc::new(Self {
            receiver: Arc::new(Mutex::new(rx)),
            sender: Arc::new(tx),
            scher: Arc::new(RwLock::new(None)),
        });

        queue
    }

    pub fn init(&self, engine: &Engine) {
        let scher = engine.scher();
        *self.scher.write().unwrap() = Some(scher.clone());
    }

    pub async fn next(&self) -> Option<Signal> {
        let receiver = &mut *self.receiver.lock().await;
        receiver.recv().await
    }

    pub(crate) fn send(&self, sig: &Signal) {
        let sender = self.sender.clone();
        let sig = sig.clone();
        Handle::current().spawn(async move { sender.send(sig).await });
    }

    pub fn terminate(&self) {
        self.send(&Signal::Terminal);
    }
}
