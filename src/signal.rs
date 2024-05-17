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

    pub fn double(&self) -> (Self, Self) {
        (self.clone(), self.clone())
    }

    pub fn triple(&self) -> (Self, Self, Self) {
        (self.clone(), self.clone(), self.clone())
    }

    pub async fn recv(&self) -> T {
        self.sig.notified().await;
        let v = self.data.lock().unwrap();
        v.clone()
    }

    pub async fn timeout(&self, millis: u64) -> T {
        tokio::time::sleep(tokio::time::Duration::from_millis(millis)).await;
        let v = self.data.lock().unwrap();
        v.clone()
    }
}

#[cfg(test)]
mod tests {
    use crate::Signal;

    #[test]
    fn engine_signal_new() {
        let s = Signal::new(5);
        assert_eq!(s.data(), 5);

        let s = Signal::new("abc");
        assert_eq!(s.data(), "abc");

        let s = Signal::new(());
        assert_eq!(s.data(), ());
    }

    #[tokio::test]
    async fn engine_signal_send() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.send(10);
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 10);
    }

    #[tokio::test]
    async fn engine_signal_close() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.close();
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 0);
    }

    #[tokio::test]
    async fn engine_signal_update() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.update(|data| *data = 100);
            s.close();
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 100);
    }

    #[tokio::test]
    async fn engine_signal_timeout() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.update(|data| *data = 100);
        });
        let ret = s2.timeout(10).await;
        assert_eq!(ret, 100);
    }

    #[tokio::test]
    async fn engine_signal_double() {
        let (s1, s2) = Signal::new(0).double();
        tokio::spawn(async move {
            s1.send(10);
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 10);
    }

    #[tokio::test]
    async fn engine_signal_triple() {
        let (s1, s2, s3) = Signal::new(0).triple();
        tokio::spawn(async move {
            s1.update(|data| *data = 100);
            s2.close();
        });
        let ret = s3.recv().await;
        assert_eq!(ret, 100);
    }
}
