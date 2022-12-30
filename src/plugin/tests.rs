use crate::{plugin, ActPlugin, Engine, Message, Workflow};
use std::sync::{Arc, Mutex};

#[test]
fn plugin_register() {
    let engine = Engine::new();

    let plugin_count = engine.plugins.lock().unwrap().len();
    engine.register_plugin(&TestPlugin::new());
    assert_eq!(engine.plugins.lock().unwrap().len(), plugin_count + 1);
}

#[tokio::test]
async fn plugin_init() {
    let engine = Engine::new();

    let test_plugin = TestPlugin::new();
    engine.register_plugin(&test_plugin);
    plugin::init(&engine).await;
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
        // engine.register_action("func", func);
        engine.on_workflow_start(|_w: &Workflow| {});
        engine.on_workflow_complete(|_w: &Workflow| {});

        engine.on_message(|_msg: &Message| {});
    }
}
