use crate::{plugin, ActPlugin, Engine};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn plugin_register() {
    let engine = Engine::new();
    let extender = engine.extender();
    let plugin_count = engine.plugins().lock().unwrap().len();
    extender.register_plugin(&TestPlugin::new());
    assert_eq!(engine.plugins().lock().unwrap().len(), plugin_count + 1);
}

#[tokio::test]
async fn plugin_init() {
    let engine = Engine::new();

    let test_plugin = TestPlugin::new();
    engine.extender().register_plugin(&test_plugin);
    plugin::init(&engine);
    assert_eq!(*test_plugin.is_init.lock().unwrap(), true);
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
