use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone)]
pub struct Signal<T> {
    data: Arc<Mutex<T>>,
    sig: Arc<Notify>,
    is_closed: Arc<Mutex<bool>>,
}

impl<T: Clone> Signal<T> {
    pub fn new(v: T) -> Self {
        let sig = Arc::new(Notify::new());
        let data = Arc::new(Mutex::new(v));
        Self {
            sig,
            data,
            is_closed: Arc::new(Mutex::new(false)),
        }
    }

    pub fn send(&self, v: T) {
        if *self.is_closed.lock().unwrap() {
            return;
        }
        *self.data.lock().unwrap() = v;
        self.close();
    }

    pub fn close(&self) {
        *self.is_closed.lock().unwrap() = true;
        self.sig.notify_one();
    }
    pub fn data(&self) -> T {
        let data = self.data.lock().unwrap();
        data.clone()
    }

    pub fn update<F: Fn(&mut T)>(&self, f: F) {
        if *self.is_closed.lock().unwrap() {
            return;
        }
        let mut data = self.data.lock().unwrap();
        f(&mut data);
    }

    pub async fn recv(&self) -> T {
        self.sig.notified().await;
        let v = self.data.lock().unwrap();
        v.clone()
    }
}
