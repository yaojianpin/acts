use crate::sch::queue::Signal;
use std::sync::Arc;
use tokio::{runtime::Handle, sync::mpsc, sync::Mutex};

#[derive(Clone)]
pub struct Queue {
    receiver: Arc<Mutex<mpsc::Receiver<Signal>>>,
    sender: Arc<mpsc::Sender<Signal>>,
}

impl Queue {
    pub fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Signal>(100);

        Arc::new(Self {
            receiver: Arc::new(Mutex::new(rx)),
            sender: Arc::new(tx),
        })
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
