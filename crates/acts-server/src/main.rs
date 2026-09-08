use acts::Config;
use acts_store::SledStore;
use std::{path::Path, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::create(Path::new("config/acts.toml"));
    init_log(&config);

    let db = config.get::<acts_server::DbConfig>("db")?;
    let store: Arc<dyn acts::KvStore> = Arc::new(SledStore::open(&db.database_url)?);
    let engine = Arc::new(
        acts_server::build_engine(&config, store, &acts_server::ServerPlugins::full())
            .start()
            .await?,
    );

    print_logo();

    let signal = engine.signal(());
    signal.recv().await;
    Ok(())
}

fn init_log(#[allow(unused_variables)] config: &Config) {
    use time::macros::format_description;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::time::LocalTime;
    use tracing_subscriber::fmt::writer::MakeWriterExt;

    const ACTS_ENV_LOG: &str = "ACTS_LOG";

    let file_appender = tracing_appender::rolling::hourly(&config.log().dir, "acts.log");
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
