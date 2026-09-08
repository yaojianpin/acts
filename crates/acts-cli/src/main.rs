mod cli;
mod client;
mod cmd;
mod util;

use clap::Parser;
use cli::Cli;
use cmd::CommandRunner;
use owo_colors::OwoColorize;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let mut port: u16 = 10080;
    let mut hostname = "127.0.0.1";

    if let Some(h) = &cli.host {
        hostname = h;
    }

    if let Some(p) = cli.port {
        port = p;
    }

    let uri = format!("http://{hostname}:{port}");
    let tip = format!("{}:{} $ ", hostname, port);
    let mut client = client::connect(&uri).await?;
    let mut cmd = CommandRunner::new(&mut client);
    show_help_tip();
    loop {
        let line = readline(&tip)?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match cmd.run(line).await {
            Ok(quit) => {
                if quit {
                    break;
                }
            }
            Err(err) => {
                writeln!(std::io::stderr(), "{}", err.red()).map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

fn readline(tip: &str) -> Result<String, String> {
    write!(std::io::stdout(), "{tip}").map_err(|e| e.to_string())?;
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut buffer = String::new();
    std::io::stdin()
        .read_line(&mut buffer)
        .map_err(|e| e.to_string())?;
    Ok(buffer)
}

fn show_help_tip() {
    let text = "tap 'help' to list available subcommands and some concept guides";
    println!("{text}");
}
