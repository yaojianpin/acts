use acts::{ActPlugin, Engine};
use axum::{
    Router,
    routing::{get, post},
};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

mod config;
mod objects;
mod routes;
mod sse;

pub use config::HttpConfig;

#[derive(Clone)]
pub struct WebPlugin;

impl WebPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ActPlugin for WebPlugin {
    fn on_init(&self, engine: &Engine) -> acts::Result<()> {
        let engine = Arc::new(engine.clone());
        let config = engine.config();
        let web_config = config.get::<HttpConfig>("web").unwrap_or_default();
        let port = web_config.port.unwrap_or(10082);
        let addr: SocketAddr = format!("0.0.0.0:{port}")
            .parse()
            .map_err(|e| acts::ActError::Config(format!("invalid web bind address: {e}")))?;

        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/hooks/{event_id}", post(routes::hook))
            .nest(
                "/api",
                Router::new()
                    .route("/model/list", post(routes::list))
                    .route("/model/get", post(routes::get))
                    .route("/model/rm", post(routes::rm))
                    .route("/model/deploy", post(routes::deploy))
                    .route("/proc/start", post(routes::proc_start))
                    .route("/pack/list", post(routes::pack_list))
                    .route("/pack/catalogs", get(routes::pack_catalogs))
                    .route("/pack", post(routes::pack_get))
                    .route("/msg/sse", get(sse::sse))
                    .route("/msg/ack", post(sse::ack))
                    .route("/snap/upsert", post(routes::snap_upsert))
                    .route("/snap/remove", post(routes::snap_remove))
                    .route("/snap/get", post(routes::snap_get))
                    .route("/snap/ls", post(routes::snap_ls)),
            )
            .with_state(engine.clone());

        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    info!(addr = %addr, "The Web server is now ready to accept connections");
                    if let Err(err) = axum::serve(listener, app).await {
                        tracing::error!(addr = %addr, error = %err, "web server stopped");
                    }
                }
                Err(err) => {
                    tracing::error!(addr = %addr, error = %err, "failed to bind web server");
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests;
