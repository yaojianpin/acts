use acts_cli::{
    cli::Cli,
    client,
    cmd::{self, CommandRunner},
};
use clap::Parser;
use owo_colors::OwoColorize;
use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    // Resolve an identity before entering the REPL: an explicit token, a
    // stored session (refreshed when its access token expired), or a login as
    // `--user`. A caller with none is anonymous, and the server answers it
    // the catalogue reads only — said out loud rather than discovered on the
    // first command.
    let (mut client, who) =
        client::connect_and_authenticate(&uri, cli.token, cli.user, cli.password).await?;
    if who.is_null() {
        println!(
            "connected anonymously to {uri}: the catalogue reads are available, nothing else. \
             Log in with 'auth login <user>' or --user."
        );
    } else {
        println!("authenticated as {}", client::identity(&who));
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
            // one rendering rule for a failed line: clap's own diagnostic for
            // a line that did not parse, the whole chain otherwise
            Err(err) => {
                writeln!(std::io::stderr(), "{}", cmd::error_text(&err).red())?;
            }
        }
    }

    Ok(())
}

/// Read one line from stdin, reporting a prompt or a read that fails instead
/// of panicking the REPL.
fn readline(tip: &str) -> anyhow::Result<String> {
    write!(std::io::stdout(), "{tip}")
        .map_err(|err| anyhow::anyhow!("failed to write the prompt: {err}"))?;
    std::io::stdout()
        .flush()
        .map_err(|err| anyhow::anyhow!("failed to flush the prompt: {err}"))?;
    let mut buffer = String::new();
    std::io::stdin()
        .read_line(&mut buffer)
        .map_err(|err| anyhow::anyhow!("failed to read the command line: {err}"))?;
    Ok(buffer)
}

fn show_help_tip() {
    let text = "tap 'help' to list available subcommands and some concept guides";
    println!("{text}");
}
