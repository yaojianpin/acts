mod pack1;
mod pack2;
mod pack3;

use acts::{ActPlugin, Engine};

pub use pack3::Pack3;

#[derive(Clone)]
pub struct MyPackagePlugin;

impl ActPlugin for MyPackagePlugin {
    fn on_init(&self, engine: &Engine) {
        engine
            .extender()
            .register_package::<pack1::Pack1>()
            .expect("failed to register Pack1");
        engine
            .extender()
            .register_package::<pack2::Pack2>()
            .expect("failed to register Pack1");

        engine
            .extender()
            .register_package::<pack3::Pack3>()
            .expect("failed to register Pack3");
    }
}
