use crate::{ActPlugin, Engine, EngineBuilder};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn plugin_register() {
    let test_plugin = TestPlugin::new();
    EngineBuilder::new()
        .add_plugin(&test_plugin)
        .build()
        .start();
    assert!(*test_plugin.is_init.lock().unwrap());
}

#[derive(Debug, Clone)]
struct TestPlugin {
    is_init: Arc<Mutex<bool>>,
}

impl TestPlugin {
    fn new() -> Self {
        Self {
            is_init: Arc::new(Mutex::new(false)),
        }
    }
}

impl ActPlugin for TestPlugin {
    fn on_init(&self, engine: &Engine) {
        println!("TestPlugin");
        *self.is_init.lock().unwrap() = true;

        // engine.register_module("name", module);
        let emitter = engine.channel();
        emitter.on_start(|_w| {});
        emitter.on_complete(|_w| {});

        emitter.on_message(|_msg| {});
    }
}
