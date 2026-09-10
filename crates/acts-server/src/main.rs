use acts::Config;
use std::{path::Path, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Config home: ~/.acts (or $ACTS_CONFIG_DIR). The default acts.toml is
    // auto-created there on first start (embedded at build time), so a binary
    // installed with `cargo install acts-server` works with zero setup.
    let config_dir = acts_server::config_dir();
    let config_file = acts_server::ensure_default_config(&config_dir)?;

    // A local `./acts.toml` in the working directory overrides the ~/.acts
    // defaults with a deep per-key merge.
    let mut config = Config::create(&config_file)?;
    config.overlay_file(Path::new("acts.toml"))?;

    init_log(&config);
    println!("config file: {}", config_file.display());

    let db = if config.has("db") {
        config.get::<acts_server::DbConfig>("db")?
    } else {
        acts_server::DbConfig::default()
    };
    let store = acts_server::open_store(&config_dir, &db).await?;
    let engine = Arc::new(
        acts_server::engine_builder(&config, store, &acts_server::ServerPlugins::full())
            .start()
            .await?,
    );

    print_logo();

    let signal = engine.signal(());
    signal.recv().await;
    Ok(())
}

fn init_log(#[allow(unused_variables)] config: &Config) {
    use std::path::Path;
    use time::macros::format_description;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::time::LocalTime;
    use tracing_subscriber::fmt::writer::MakeWriterExt;

    const ACTS_ENV_LOG: &str = "ACTS_LOG";

    let log_dir = config.log().dir;
    std::fs::create_dir_all(&log_dir)
        .unwrap_or_else(|err| panic!("failed to create log dir {log_dir}: {err}"));
    let log_dir = Path::new(&log_dir);
    let file_appender = tracing_appender::rolling::hourly(log_dir, "acts.log");
    let timer = LocalTime::new(format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:9]"
    ));
    unsafe {
        std::env::set_var(ACTS_ENV_LOG, &config.log().level);
    }

    tracing_subscriber::fmt()
        .with_timer(timer)
        .with_env_filter(EnvFilter::from_env(ACTS_ENV_LOG))
        .with_writer(std::io::stdout.and(file_appender))
        .with_ansi(false)
        .init();
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
