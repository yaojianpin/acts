use acts::{ActError, ActPlugin, Engine, resolve_level};
use axum::serve::ListenerExt;
use axum::{
    Router, middleware,
    routing::{get, post},
};
use limit::{LimitState, WebTransport};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use tracing::info;

mod auth;
mod config;
mod limit;
mod objects;
mod routes;
mod sse;
pub use config::{
    DEFAULT_HOST, DEFAULT_MAX_CONCURRENT, DEFAULT_MAX_CONCURRENT_PER_IP, DEFAULT_QUEUE_SIZE,
    DEFAULT_RATE_LIMIT_PER_MIN, HttpConfig, TlsConfig,
};

/// The posture verdict for a plaintext (non-TLS) bind.
#[derive(Debug)]
pub enum PlaintextPosture {
    /// Nothing to say about this bind.
    Allow,
    /// Allowed, but the operator should know what they accepted.
    Warn(String),
    /// The start fails with this reason.
    Refuse(String),
}

#[derive(Clone)]
pub struct WebPlugin {
    /// The serving task, so `shutdown` can join it. Clones share the handle
    /// (the builder boxes a clone of the plugin the caller passed).
    task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

/// Every `BEGIN`/`END` CERTIFICATE block of a PEM file (a chain file holds
/// the server cert followed by its intermediates).
fn pem_certs(path: &str) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    let certs = rustls::pki_types::CertificateDer::pem_file_iter(path)
        .map_err(|e| format!("cannot read '{path}': {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("'{path}' is not readable PEM: {e}"))?;
    if certs.is_empty() {
        Err(format!("'{path}' holds no certificate"))
    } else {
        Ok(certs)
    }
}

/// The first private key of a PEM file.
fn pem_key(path: &str) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    use rustls::pki_types::PrivateKeyDer;
    use rustls::pki_types::pem::PemObject;
    PrivateKeyDer::from_pem_file(path)
        .map_err(|e| format!("'{path}' is not a readable private key PEM: {e}"))
}

/// The listener that terminates TLS before handing the stream to axum: axum's
/// `serve` accepts any [`axum::serve::Listener`], so wrapping the accept loop
/// needs no second HTTP stack.
struct TlsListener {
    inner: tokio::net::TcpListener,
    acceptor: Arc<TlsAcceptor>,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => match self.acceptor.accept(stream).await {
                    Ok(tls) => return (tls, addr),
                    // A failed handshake (client speaks plain HTTP, bad
                    // SNI, closed mid-way) loses one connection, never the
                    // listener.
                    Err(err) => tracing::debug!(addr = %addr, error = %err, "tls handshake failed"),
                },
                Err(err) => {
                    tracing::warn!(error = %err, "web listener accept failed");
                    // Fatal accept errors (EMFILE storms aside) are
                    // transient; back off instead of spinning.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

impl WebPlugin {
    pub fn new() -> Self {
        Self {
            task: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// The configured bind address, parsed and validated.
    fn address(engine: &Engine) -> acts::Result<SocketAddr> {
        let config = engine.config();
        let web_config = config.get::<HttpConfig>("web").unwrap_or_default();
        let host = web_config
            .host
            .clone()
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = web_config.port.unwrap_or(10082);
        format!("{host}:{port}").parse().map_err(|e| {
            acts::ActError::Config(format!("invalid web bind address {host}:{port}: {e}"))
        })
    }

    /// The posture verdict for a plaintext bind, by effective level:
    ///
    /// - `permissive` — a non-loopback plaintext bind only warns: the
    ///   deployment said it accepts the risk;
    /// - `standard` — refused unless `tls` is configured or the deployment
    ///   explicitly opted out with `plain_behind_proxy`;
    /// - `strict` — refused unless `tls` is configured; `plain_behind_proxy`
    ///   does not rescue it, because a proxy hop inside the perimeter is
    ///   exactly the hop strict mode refuses to trust.
    pub fn plaintext_posture(
        level: acts::Level,
        addr: SocketAddr,
        has_tls: bool,
        plain_behind_proxy: bool,
    ) -> PlaintextPosture {
        if addr.ip().is_loopback() || has_tls {
            return PlaintextPosture::Allow;
        }
        match level {
            acts::Level::Permissive => PlaintextPosture::Warn(format!(
                "serving plain HTTP on {addr} at posture level 'permissive': credentials \
                 cross the network unencrypted"
            )),
            acts::Level::Standard if plain_behind_proxy => PlaintextPosture::Allow,
            acts::Level::Standard => PlaintextPosture::Refuse(format!(
                "refusing to bind the web server on {addr}: a non-loopback address needs \
                 [web].tls or an explicit plain_behind_proxy = true (plain HTTP would carry \
                 credentials unencrypted)"
            )),
            acts::Level::Strict => PlaintextPosture::Refuse(format!(
                "refusing to bind the web server on {addr}: posture level 'strict' requires \
                 [web].tls on a non-loopback address (plain_behind_proxy does not satisfy it, \
                 a proxy hop is the hop strict mode refuses to trust)"
            )),
        }
    }

    /// Load the listener's rustls configuration from `[web].tls`.
    pub fn load_tls_server_config(tls: &TlsConfig) -> acts::Result<rustls::ServerConfig> {
        // The ring provider backs the handshake (no C toolchain in the build
        // graph); install it once per process — a second install only means
        // another transport got here first, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let certs =
            pem_certs(&tls.cert).map_err(|e| ActError::Config(format!("web.tls.cert: {e}")))?;
        let key = pem_key(&tls.key).map_err(|e| ActError::Config(format!("web.tls.key: {e}")))?;

        let builder = match &tls.client_ca {
            Some(ca_path) => {
                let mut roots = rustls::RootCertStore::empty();
                for cert in pem_certs(ca_path)
                    .map_err(|e| ActError::Config(format!("web.tls.client_ca: {e}")))?
                {
                    roots
                        .add(cert)
                        .map_err(|e| ActError::Config(format!("web.tls.client_ca: {e}")))?;
                }
                let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
                    .build()
                    .map_err(|e| ActError::Config(format!("web.tls.client_ca verifier: {e}")))?;
                rustls::ServerConfig::builder().with_client_cert_verifier(verifier)
            }
            None => rustls::ServerConfig::builder().with_no_client_auth(),
        };
        let mut config = builder
            .with_single_cert(certs, key)
            .map_err(|e| ActError::Config(format!("web.tls: cert and key do not match: {e}")))?;
        // The default protocol versions (TLS 1.2 + 1.3, the provider's
        // modern cipher suites) are kept; ALPN offers both HTTP versions so
        // an h2-capable client can negotiate it.
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(config)
    }

    /// The request router: every route with its per-action body cap, the auth
    /// layer, and the admission stack. Built here (not inside `start`) so the
    /// tests can drive it over a real socket without the plugin lifecycle.
    pub(crate) fn build_router(engine: &Arc<Engine>, limits: &Arc<LimitState>) -> Router {
        // Per-route raw-body caps from the action profile: the cap of
        // `model:deploy` applies to `/model/deploy`, and so on — one table
        // (`limit::route_action`) names the action of every path, so a route
        // and its budget cannot drift apart.
        let capped = |path: &'static str, router: axum::routing::MethodRouter<Arc<Engine>>| {
            let cap = limit::route_action(path)
                .map(|action| engine.ingress().profile(&action).max_raw_bytes)
                .unwrap_or(0);
            router.layer(axum::extract::DefaultBodyLimit::max(cap))
        };

        // Every route below resolves an `authorization: Bearer <token>`
        // header into a principal first; `/health` stays open for probes.
        let api = Router::new()
            .route("/model/list", capped("/model/list", post(routes::list)))
            .route("/model/get", capped("/model/get", post(routes::get)))
            .route("/model/rm", capped("/model/rm", post(routes::rm)))
            .route(
                "/model/deploy",
                capped("/model/deploy", post(routes::deploy)),
            )
            .route(
                "/proc/start",
                capped("/proc/start", post(routes::proc_start)),
            )
            .route("/act/{action}", capped("/act/x", post(routes::act)))
            .route("/pack/list", capped("/pack/list", post(routes::pack_list)))
            .route(
                "/pack/catalogs",
                capped("/pack/catalogs", get(routes::pack_catalogs)),
            )
            .route("/pack", capped("/pack", post(routes::pack_get)))
            .route("/msg/sse", capped("/msg/sse", get(sse::sse)))
            .route("/msg/ack", capped("/msg/ack", post(sse::ack)))
            .route(
                "/snap/upsert",
                capped("/snap/upsert", post(routes::snap_upsert)),
            )
            .route(
                "/snap/remove",
                capped("/snap/remove", post(routes::snap_remove)),
            )
            .route("/snap/get", capped("/snap/get", post(routes::snap_get)))
            .route("/snap/ls", capped("/snap/ls", post(routes::snap_ls)))
            .route(
                "/snap/reply",
                capped("/snap/reply", post(routes::snap_reply)),
            )
            .route_layer(middleware::from_fn_with_state(
                engine.clone(),
                auth::require_auth,
            ));
        let api = limit::apply_layers(limits, api);

        // Only the hook endpoint needs the layer here: `route_layer` covers
        // every route added *before* it, so `/health` is registered last and
        // stays open for probes.
        let hooks = Router::new()
            .route("/hooks/{event_id}", capped("/hooks/x", post(routes::hook)))
            .route_layer(middleware::from_fn_with_state(
                engine.clone(),
                auth::require_auth,
            ));
        let hooks = limit::apply_layers(limits, hooks);

        hooks
            .nest("/api", api)
            .route("/health", get(|| async { "ok" }))
            .with_state(engine.clone())
    }
}

impl Default for WebPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ActPlugin for WebPlugin {
    /// Validate the bind address and the route table; nothing listens yet.
    async fn prepare(&self, engine: &Engine) -> acts::Result<()> {
        Self::address(engine)?;
        Ok(())
    }

    async fn start(&self, engine: &Engine) -> acts::Result<()> {
        let engine = Arc::new(engine.clone());
        let addr = Self::address(&engine)?;
        let shutdown = engine.shutdown_token();
        let web_config: HttpConfig = engine.config().get::<HttpConfig>("web").unwrap_or_default();

        // Admission state (limits, rate buckets, the inflight counter) and
        // the lifecycle handle that watches it are built before anything
        // listens, so a start that fails here never registered a transport
        // that would drain nothing.
        let limits = LimitState::new(&engine, &web_config)?;
        let tls = match &web_config.tls {
            Some(tls_config) => Some(Arc::new(TlsAcceptor::from(Arc::new(
                Self::load_tls_server_config(tls_config)?,
            )))),
            None => None,
        };
        // the plaintext rule follows the posture level ([web].level >
        // [security].level > standard)
        let level = resolve_level(web_config.level.as_deref(), &engine.config());
        match Self::plaintext_posture(level, addr, tls.is_some(), web_config.plain_behind_proxy) {
            PlaintextPosture::Allow => {}
            PlaintextPosture::Warn(why) => tracing::warn!(addr = %addr, "{why}"),
            PlaintextPosture::Refuse(why) => return Err(ActError::Config(why)),
        }

        let app = Self::build_router(&engine, &limits);

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

        // Register before serving: from the first accepted connection on, a
        // close quiesces the gate (new requests 503) and drains what it
        // admitted before the plugin's shutdown joins the accept loop.
        engine.register_transport(Arc::new(WebTransport::new(limits.counter.clone())));

        info!(addr = %addr, tls = tls.is_some(), "The Web server is now ready to accept connections");
        let handle = tokio::spawn(async move {
            // `into_make_service_with_connect_info` hands every request its
            // peer address — the per-IP limits and the SSE subscriptions
            // attribute their quotas to it.
            let serve = match tls {
                Some(acceptor) => {
                    // `tap_io` is what makes the TLS stream carry its peer
                    // address into `ConnectInfo`: it is the wrapper axum's
                    // make-service knows how to read `Connected` from.
                    axum::serve(
                        TlsListener {
                            inner: listener,
                            acceptor,
                        }
                        .tap_io(|_| {}),
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(async move { shutdown.cancelled().await })
                    .await
                }
                None => {
                    axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(async move { shutdown.cancelled().await })
                    .await
                }
            };
            if let Err(err) = serve {
                tracing::error!(addr = %addr, error = %err, "web server stopped");
            } else {
                info!(addr = %addr, "web server stopped");
            }
        });
        let mut slot = self.task.lock().await;
        *slot = Some(handle);

        Ok(())
    }

    /// The engine cancels the shutdown token before it calls this; joining the
    /// serving task is what proves the port is released.
    async fn shutdown(&self) -> acts::Result<()> {
        let handle = self.task.lock().await.take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
