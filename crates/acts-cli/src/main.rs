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
    let mut client = client::connect(&uri, cli.token).await?;

    // Resolve the identity before entering the REPL: a missing or stale token
    // is a startup problem, not a surprise on the first command.
    match client::whoami(&mut client).await {
        Ok(who) => {
            let subject = who["subject"].as_str().unwrap_or("?");
            let roles = who["roles"]
                .as_array()
                .map(|roles| {
                    roles
                        .iter()
                        .filter_map(|role| role.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let identity = if who["unrestricted"].as_bool().unwrap_or(false) {
                "unrestricted".to_string()
            } else if roles.is_empty() {
                format!("subject {subject}")
            } else {
                format!("roles: {roles}")
            };
            println!("authenticated as {identity}");
        }
        Err(err) => {
            let hint = if std::env::var("ACTS_TOKEN").is_err() {
                " (no token given: use --token or ACTS_TOKEN)"
            } else {
                " (check ACTS_TOKEN / --token)"
            };
            writeln!(std::io::stderr(), "{}{}", err.to_string().red(), hint)?;
            std::process::exit(1);
        }
    }

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
