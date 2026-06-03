use tokio::runtime::Runtime;

pub fn block_on<F: Future>(f: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(f)),
        Err(_) => {
            let runtime = Runtime::new().expect("failed to create tokio runtime");
            runtime.block_on(f)
        }
    }
}
