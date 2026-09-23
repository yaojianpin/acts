use acts::{ActPlugin, Engine};
use axum::{
    Router, middleware,
    routing::{get, post},
};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

mod auth;
mod config;
mod objects;
mod routes;
mod sse;
pub use config::{DEFAULT_HOST, DEFAULT_QUEUE_SIZE, HttpConfig};

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
        let host = web_config
            .host
            .clone()
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = web_config.port.unwrap_or(10082);
        let addr: SocketAddr = format!("{host}:{port}").parse().map_err(|e| {
            acts::ActError::Config(format!("invalid web bind address {host}:{port}: {e}"))
        })?;
        let shutdown = engine.shutdown_token();

        // Every route below resolves an `authorization: Bearer <token>`
        // header into a principal first; `/health` stays open for probes.
        let api = Router::new()
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
            .route("/snap/ls", post(routes::snap_ls))
            .route_layer(middleware::from_fn_with_state(
                engine.clone(),
                auth::require_auth,
            ));

        // Only the hook endpoint needs the layer here: `route_layer` covers
        // every route added *before* it, so `/health` is registered last and
        // stays open for probes.
        let app = Router::new()
            // axum 0.8 path syntax: `{param}`, not `:param`.
            .route("/hooks/{event_id}", post(routes::hook))
            .route_layer(middleware::from_fn_with_state(
                engine.clone(),
                auth::require_auth,
            ))
            .nest("/api", api)
            .route("/health", get(|| async { "ok" }))
            .with_state(engine.clone());

        // The listener is bound here, not inside the spawned task: a bind
        // failure (port taken, address unavailable) is a deployment fault that
        // must fail the engine start with its reason, instead of logging and
        // leaving a "running" engine whose HTTP transport never came up.
        let listener = std::net::TcpListener::bind(addr).map_err(|e| {
            acts::ActError::Config(format!("failed to bind web server on {addr}: {e}"))
        })?;
        listener.set_nonblocking(true).map_err(|e| {
            acts::ActError::Config(format!(
                "failed to configure the web listener on {addr}: {e}"
            ))
        })?;
        let listener = tokio::net::TcpListener::from_std(listener).map_err(|e| {
            acts::ActError::Config(format!(
                "failed to register the web listener on {addr}: {e}"
            ))
        })?;

        info!(addr = %addr, "The Web server is now ready to accept connections");
        tokio::spawn(async move {
            let serve = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled().await });
            if let Err(err) = serve.await {
                tracing::error!(addr = %addr, error = %err, "web server stopped");
            } else {
                info!(addr = %addr, "web server stopped");
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests;
