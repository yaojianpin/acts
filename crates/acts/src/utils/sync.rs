use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to create tokio runtime"))
}

pub fn block_on<F: Future>(f: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_handle) => {
            // Inside a tokio runtime — use block_in_place to avoid blocking
            // worker threads, but run the future on a dedicated separate runtime
            // so that sqlx pool operations don't compete with the parent runtime.
            tokio::task::block_in_place(|| runtime().block_on(f))
        }
        Err(_) => runtime().block_on(f),
    }
}
