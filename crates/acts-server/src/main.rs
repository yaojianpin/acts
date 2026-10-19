use acts_server::{Config, ServerPlugins};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Config home: ~/.acts (or $ACTS_CONFIG_DIR). The default acts.toml is
    // auto-created there on first start (embedded at build time), so a binary
    // installed with `cargo install acts-server` works with zero setup.
    let config_dir = acts_server::config_dir();
    let config_file = acts_server::ensure_default_config(&config_dir)?;

    // A local `./acts.toml` in the working directory overrides the ~/.acts
    // defaults with a deep per-key merge.
    let mut config = Config::create(&config_file)?;
    config.overlay_file(Path::new("acts.toml"))?;

    init_log(&config)?;
    println!("config file: {}", config_file.display());

    serve(config_dir, config, ServerPlugins::full(), shutdown_signal()).await
}

/// Open the store, start the engine, and run until `shutdown` resolves — the
/// body of [`main`] after the config is loaded. Extracted so the startup and
/// shutdown path can be exercised without spawning the binary for a full
/// process lifetime.
async fn serve<F>(
    config_dir: PathBuf,
    config: Config,
    plugins: ServerPlugins,
    shutdown: F,
) -> Result<(), anyhow::Error>
where
    F: Future<Output = ()>,
{
    let db = if config.has("db") {
        config.get::<acts_server::DbConfig>("db")?
    } else {
        acts_server::DbConfig::default()
    };
    // Opening the store also takes the database's exclusive lease: a second
    // server on the same database is refused here, before an engine exists to
    // duplicate recovery and scheduling.
    let opened = acts_server::open_store(&config_dir, &db).await?;
    let started = match acts_server::engine_builder(&config, opened.store.clone(), &plugins) {
        Ok(builder) => builder.start().await,
        Err(err) => Err(err),
    };
    let engine = match started {
        Ok(engine) => Arc::new(engine),
        Err(err) => {
            // A startup that failed hands the lease back: fixing the config and
            // restarting must not wait out the TTL.
            if let Some(lease) = opened.lease()
                && let Err(release) = lease.release().await
            {
                tracing::warn!(error = %release, "failed to release the database lease");
            }
            return Err(err.into());
        }
    };
    // The lease is renewed while the engine runs, and the engine is stopped if
    // it is lost (every write is refused from that point on).
    let keeper = opened.keep_lease(&engine);

    print_logo();

    shutdown.await;
    info!("shutdown signal received, closing the engine");
    engine.close().await;
    if let Some(keeper) = keeper {
        // Waits for the release, so the successor of a rolling update starts
        // at once instead of waiting out the lease TTL.
        keeper.stop().await;
    }
    info!("engine closed, exiting");

    Ok(())
}

/// Resolve when the process is asked to stop: Ctrl-C (SIGINT) everywhere,
/// plus SIGTERM on unix so a deployment's rolling update drains the engine
/// instead of force-killing it.
///
/// A handler that cannot be installed is a deployment fault, but not one that
/// justifies killing a server whose engine is healthy: the failure is logged,
/// that signal stops being a shutdown trigger, and every other one still is —
/// a supervisor's SIGTERM or SIGKILL included.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to install the Ctrl-C handler; Ctrl-C will not stop the server");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                error!(error = %err, "failed to install the SIGTERM handler; SIGTERM will not drain the engine");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

fn init_log(#[allow(unused_variables)] config: &Config) -> Result<(), anyhow::Error> {
    use anyhow::Context;
    use time::macros::format_description;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::time::LocalTime;
    use tracing_subscriber::fmt::writer::MakeWriterExt;

    const ACTS_ENV_LOG: &str = "ACTS_LOG";

    let log = config.log();
    let file_appender = acts_server::log_file_appender(&log)
        .with_context(|| format!("failed to open the log file under {}", log.dir))?;
    let timer = LocalTime::new(format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:9]"
    ));
    unsafe {
        std::env::set_var(ACTS_ENV_LOG, &log.level);
    }

    tracing_subscriber::fmt()
        .with_timer(timer)
        .with_env_filter(EnvFilter::from_env(ACTS_ENV_LOG))
        .with_writer(std::io::stdout.and(file_appender))
        .with_ansi(false)
        .init();

    Ok(())
}

fn print_logo() {
    let version = env!("CARGO_PKG_VERSION");
    let banner = format!(
        r#"

    █████╗  ███████╗████████╗███████╗
    ██╔══██╗██╔════╝╚══██╔══╝██╔════╝
    ███████║██║        ██║   ███████║
    ██╔══██║██║        ██║   ╚════██║
    ██║  ██║╚██████╗   ██║   ███████║
    ╚═╝  ╚═╝ ╚═════╝   ╚═╝   ╚══════╝ v{version}

    Acts Workflow Engine
    "#
    );
    println!("{banner}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts_server::DbType;

    /// Scratch config dir for one case, wiped so it starts empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("acts-main-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn load(dir: &Path, body: &str) -> Config {
        let path = dir.join(acts_server::CONFIG_FILE_NAME);
        std::fs::write(&path, body).unwrap();
        Config::create(&path).unwrap()
    }

    fn sqlite_db(dir: &Path) -> (acts_server::DbConfig, String) {
        let url = dir.join("acts.db").to_string_lossy().into_owned();
        (
            acts_server::DbConfig {
                kind: DbType::Sqlite,
                database_url: Some(url.clone()),
                lease_owner: Some("main-test".to_string()),
                ..acts_server::DbConfig::default()
            },
            url,
        )
    }

    /// With no `[db]` section the server runs on the default sled database
    /// under the config dir, takes the lease, and hands it back on shutdown.
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_defaults_to_sled_and_shuts_down() {
        let dir = scratch("default");
        let config = load(&dir, "");

        serve(dir.clone(), config, ServerPlugins::default(), async {})
            .await
            .unwrap();
        assert!(dir.join("data").is_dir(), "the sled data dir exists");

        // sled may hold its file lock briefly after the store is dropped, so
        // the lease release itself is asserted on the sqlite case below.
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A configured database is used, the engine starts, and a graceful
    /// shutdown closes it.
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_uses_the_configured_database() {
        let dir = scratch("configured");
        let (db, url) = sqlite_db(&dir);
        let config = load(
            &dir,
            &format!("[db]\ntype = \"sqlite\"\ndatabase_url = '{url}'\n"),
        );

        serve(dir.clone(), config, ServerPlugins::default(), async {})
            .await
            .unwrap();
        assert!(Path::new(&url).exists(), "the sqlite file exists");

        let reopened = acts_server::open_store(&dir, &db).await.unwrap();
        assert_eq!(reopened.lease().unwrap().fence(), 2);
        reopened.lease().unwrap().release().await.unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A startup that fails after taking the lease hands it back, so fixing
    /// the config and restarting does not wait out the TTL.
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_releases_the_lease_when_the_engine_fails() {
        let dir = scratch("engine-fail");
        let (db, url) = sqlite_db(&dir);
        // `snapshot` is a scalar where the server expects an array of tables:
        // engine construction fails with a config error.
        let config = load(
            &dir,
            &format!("snapshot = \"bad\"\n[db]\ntype = \"sqlite\"\ndatabase_url = '{url}'\n"),
        );

        let err = serve(dir.clone(), config, ServerPlugins::default(), async {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("snapshot"), "got: {err}");

        let reopened = acts_server::open_store(&dir, &db).await.unwrap();
        assert_eq!(
            reopened.lease().unwrap().fence(),
            2,
            "the failed startup released the lease"
        );
        reopened.lease().unwrap().release().await.unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `lease = false` starts without a keeper, so shutdown has nothing to
    /// stop.
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_without_the_lease_shuts_down() {
        let dir = scratch("no-lease");
        let (_db, url) = sqlite_db(&dir);
        let config = load(
            &dir,
            &format!("[db]\ntype = \"sqlite\"\ndatabase_url = '{url}'\nlease = false\n"),
        );

        serve(dir.clone(), config, ServerPlugins::default(), async {})
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shutdown future waits for a signal instead of resolving on its own;
    /// the task is aborted once the handlers are installed.
    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_signal_waits() {
        let handle = tokio::spawn(shutdown_signal());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!handle.is_finished(), "shutdown must wait for a signal");
        handle.abort();
    }
}
