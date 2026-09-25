use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "acts-cli")]
#[command(about = "cli for acts-server", long_about = None, styles=crate::util::CLAP_STYLING)]
pub struct Cli {
    #[arg(long)]
    pub host: Option<String>,

    #[arg(short, long)]
    pub port: Option<u16>,

    /// ACL token presented on every request. Takes precedence over the
    /// ACTS_TOKEN environment variable, which is the safer place for it: an
    /// argument is visible to every process on the machine.
    #[arg(long, env = "ACTS_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// Log in as this user before entering the REPL. The password comes from
    /// --password / ACTS_PASSWORD, or is prompted for. A stored session
    /// (`auth login`) is reused first, and refreshed when its token expired.
    #[arg(short, long, env = "ACTS_USER")]
    pub user: Option<String>,

    /// Password for --user. ACTS_PASSWORD is the safer place for it.
    #[arg(long, env = "ACTS_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,
}
