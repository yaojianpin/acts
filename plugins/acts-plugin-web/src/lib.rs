use acts::{ActPlugin, Engine};
use axum::{
    Router,
    routing::{get, post},
};
use std::net::SocketAddr;
use std::sync::Arc;

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
        let web_config = config.get::<HttpConfig>("http").unwrap_or_default();
        let port = web_config.port.unwrap_or(10082);
        let addr = format!("0.0.0.0:{port}")
            .parse::<SocketAddr>()
            .map_err(|e| acts::ActError::Config(e.to_string()))?;

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
                    .route("/msg/ack", post(sse::ack)),
            )
            .with_state(engine.clone());

        tokio::spawn(async move {
            println!(
                "The Web server is now ready to accept connections on port {}",
                port
            );
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests;
