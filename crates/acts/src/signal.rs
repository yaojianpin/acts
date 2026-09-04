use parking_lot::Mutex;
use std::{fmt::Debug, sync::Arc};
use tokio::sync::watch;

/// A one-shot, cloneable signal. The first `send`/`close` fires it; every
/// receiver observes the fire — receivers already waiting on `recv` are all
/// released, and receivers that start waiting afterwards return immediately.
/// This makes `double()`/`triple()` safe to use as N-way synchronization.
///
/// Locking: the value and the fired flag live under separate mutexes. `send`
/// and `update` take the value lock first and only touch the flag while
/// holding it, so the value is always written before the flag flips; `close`
/// touches only the flag lock. `close()` is therefore safe to call from
/// inside an `update` closure on the same signal. The other re-entrant calls
/// (`send`, `data`, `update` from an `update` closure) would re-lock the
/// value mutex and must not be used.
#[derive(Clone)]
pub struct Signal<T> {
    data: Arc<Mutex<T>>,
    is_closed: Arc<Mutex<bool>>,
    // Broadcast notification of the one-shot fire. Kept alive (as `tx`) for
    // the whole life of the signal so `recv` never sees the sender dropped.
    closed: watch::Receiver<bool>,
    tx: watch::Sender<bool>,
}

impl<T: Clone> Signal<T> {
    pub fn new(v: T) -> Self {
        let (tx, closed) = watch::channel(false);
        Self {
            data: Arc::new(Mutex::new(v)),
            is_closed: Arc::new(Mutex::new(false)),
            closed,
            tx,
        }
    }

    /// Fire the signal with a new value. The first call wins; later calls are
    /// no-ops. All receivers waiting on the signal wake up with `v`, and
    /// receivers that join later observe `v` immediately.
    pub fn send(&self, v: T) {
        {
            // value lock first, flag second: a receiver that observes the
            // fire can never read a stale value
            let mut data = self.data.lock();
            let mut is_closed = self.is_closed.lock();
            if *is_closed {
                return;
            }
            *data = v;
            *is_closed = true;
        }
        let _ = self.tx.send(true);
    }

    /// Fire the signal without touching the value.
    pub fn close(&self) {
        {
            let mut is_closed = self.is_closed.lock();
            if *is_closed {
                return;
            }
            *is_closed = true;
        }
        let _ = self.tx.send(true);
    }

    pub fn data(&self) -> T {
        let data = self.data.lock();
        data.clone()
    }

    pub fn update<F: Fn(&mut T)>(&self, f: F)
    where
        T: Debug,
    {
        let mut data = self.data.lock();
        if *self.is_closed.lock() {
            return;
        }
        f(&mut data);
    }

    pub fn double(&self) -> (Self, Self) {
        (self.clone(), self.clone())
    }

    pub fn triple(&self) -> (Self, Self, Self) {
        (self.clone(), self.clone(), self.clone())
    }

    /// Wait until the signal fires and return the current value. Safe to await
    /// from any number of receivers at once: the fire is broadcast to every
    /// waiter, and a receiver that joins after the fire returns immediately.
    pub async fn recv(&self) -> T {
        loop {
            let mut closed = self.closed.clone();
            if *self.is_closed.lock() {
                return self.data();
            }
            // `closed` was snapshotted before the check above, so a fire
            // landing after the check bumps a version this receiver is already
            // waiting on — no lost wakeup, unlike a plain notify.
            match closed.changed().await {
                Ok(()) => continue,
                Err(_) => return self.data(),
            }
        }
    }

    pub async fn timeout(&self, millis: u64) -> T {
        tokio::time::sleep(tokio::time::Duration::from_millis(millis)).await;
        self.data()
    }
}

#[cfg(test)]
mod tests {
    use crate::Signal;
    use serial_test::serial;
    use std::{sync::Arc, time::Duration};
    use tokio::sync::Barrier;
    use tokio::time::timeout;

    /// Spawns `n` tasks, each parked on `sig.recv()` only after every task has
    /// reached the barrier, so a fire afterwards must reach all of them.
    fn spawn_waiters<T>(
        sig: &Signal<T>,
        n: usize,
    ) -> (Arc<Barrier>, Vec<tokio::task::JoinHandle<T>>)
    where
        T: Clone + Send + 'static,
    {
        let barrier = Arc::new(Barrier::new(n + 1));
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let sig = sig.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                sig.recv().await
            }));
        }
        (barrier, handles)
    }

    async fn collect<T>(handles: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
        let mut rets = Vec::with_capacity(handles.len());
        for h in handles {
            rets.push(h.await.unwrap());
        }
        rets
    }

    #[test]
    fn engine_signal_new() {
        let s = Signal::new(5);
        assert_eq!(s.data(), 5);

        let s = Signal::new("abc");
        assert_eq!(s.data(), "abc");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_send() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.send(10);
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_close() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.close();
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
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

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_timeout() {
        let s = Signal::new(0);
        let s2 = s.clone();
        tokio::spawn(async move {
            s.update(|data| *data = 100);
        });
        let ret = s2.timeout(10).await;
        assert_eq!(ret, 100);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_double() {
        let (s1, s2) = Signal::new(0).double();
        tokio::spawn(async move {
            s1.send(10);
        });
        let ret = s2.recv().await;
        assert_eq!(ret, 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_triple() {
        let (s1, s2, s3) = Signal::new(0).triple();
        tokio::spawn(async move {
            s1.update(|data| *data = 100);
            s2.close();
        });
        let ret = s3.recv().await;
        assert_eq!(ret, 100);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_update_close_reentrant() {
        // `close` from inside an `update` closure must not deadlock
        let sig = Signal::new(Vec::new());
        let s2 = sig.clone();
        let sig2 = sig.clone();
        tokio::spawn(async move {
            s2.update(|data| {
                data.push(1);
                data.push(2);
                s2.close();
            });
        });
        let ret = timeout(Duration::from_secs(2), sig2.recv())
            .await
            .expect("close inside update must not deadlock");
        assert_eq!(ret, vec![1, 2]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_receiver_send_broadcast() {
        let sig = Signal::new(0);
        let (barrier, handles) = spawn_waiters(&sig, 8);
        barrier.wait().await;
        sig.send(10);
        let rets = timeout(Duration::from_secs(2), collect(handles))
            .await
            .expect("send must release every waiting receiver");
        assert_eq!(rets, vec![10; 8]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_receiver_close_broadcast() {
        let sig = Signal::new(3);
        let (barrier, handles) = spawn_waiters(&sig, 8);
        barrier.wait().await;
        sig.close();
        let rets = timeout(Duration::from_secs(2), collect(handles))
            .await
            .expect("close must release every waiting receiver");
        assert_eq!(rets, vec![3; 8]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_receiver_update_close_broadcast() {
        let sig = Signal::new(0);
        sig.update(|v| *v = 100);
        let (barrier, handles) = spawn_waiters(&sig, 8);
        barrier.wait().await;
        sig.close();
        let rets = timeout(Duration::from_secs(2), collect(handles))
            .await
            .expect("close after update must release every waiting receiver");
        assert_eq!(rets, vec![100; 8]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial]
    async fn engine_signal_receiver_late_join() {
        // receivers that start waiting after the fire must still observe it
        let sig = Signal::new(0);
        sig.send(7);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let sig = sig.clone();
                tokio::spawn(async move { sig.recv().await })
            })
            .collect();
        let rets = timeout(Duration::from_secs(2), collect(handles))
            .await
            .expect("recv after send must return immediately");
        assert_eq!(rets, vec![7; 8]);
    }
}
