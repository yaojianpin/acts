use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "acts-cli")]
#[command(about = "cli for acts-server", long_about = None, styles=crate::util::CLAP_STYLING)]
pub struct Cli {
    #[arg(long)]
    pub host: Option<String>,

    #[arg(short, long)]
    pub port: Option<u16>,
}
