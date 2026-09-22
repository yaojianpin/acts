use acts_server::Config;
use std::{path::Path, sync::Arc};
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

    let db = if config.has("db") {
        config.get::<acts_server::DbConfig>("db")?
    } else {
        acts_server::DbConfig::default()
    };
    // Opening the store also takes the database's exclusive lease: a second
    // server on the same database is refused here, before an engine exists to
    // duplicate recovery and scheduling.
    let opened = acts_server::open_store(&config_dir, &db).await?;
    let started = match acts_server::engine_builder(
        &config,
        opened.store.clone(),
        &acts_server::ServerPlugins::full(),
    ) {
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

    shutdown_signal().await;
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
