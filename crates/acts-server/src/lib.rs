//! Engine construction for the acts server binary — shared with the
//! integration tests so they exercise exactly what `acts-server` runs.

use acts::{Config, Engine, KvStore};
use std::sync::Arc;

/// Which plugins the built engine registers.
#[derive(Debug, Clone, Default)]
pub struct ServerPlugins {
    /// gRPC server plugin (default port 10080 unless `[grpc]` sets one).
    pub grpc: bool,
    /// web plugin.
    pub web: bool,
    /// NATS plugin — only registered when the config has a `[nats]` section.
    pub nats: bool,
}

impl ServerPlugins {
    /// Everything the shipped `acts-server` binary registers.
    pub fn full() -> Self {
        Self {
            grpc: true,
            web: true,
            nats: true,
        }
    }
}

/// Build the acts-server engine: core packages plus the transport plugins
/// selected by `plugins`. The NATS plugin is additionally gated on the
/// config having a `[nats]` section, so a server without one never tries to
/// reach a broker.
pub fn build_engine(config: &Config, store: Arc<dyn KvStore>, plugins: &ServerPlugins) -> Engine {
    let mut builder = Engine::builder().set_config(config).set_store(store);
    if plugins.grpc {
        builder = builder.add_plugin(&acts_plugin_grpc::GrpcPlugin::new());
    }
    if plugins.web {
        builder = builder.add_plugin(&acts_plugin_web::WebPlugin::new());
    }
    builder = builder
        .add_package::<acts_package_http::HttpPackage>()
        .add_package::<acts_package_shell::ShellPackage>()
        .add_package::<acts_package_state::StatePackage>()
        .add_package::<acts_package_nats::NatsPackage>();
    if plugins.nats && config.has("nats") {
        builder = builder.add_plugin(&acts_plugin_nats::NatsPlugin::new());
    }
    builder.build()
}
