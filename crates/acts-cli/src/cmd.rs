// The command modules are public so a test can drive `model get` and read the
// text the REPL would print, not just whether the command succeeded.
pub mod act;
pub mod evt;
pub mod model;
pub mod msg;
pub mod pack;
pub mod proc;
pub mod snap;
pub mod task;

use crate::client;
use act::ActArgs;
use acts_channel::{ActionResult, ActsChannel, Vars, model::Package};
use clap::{Parser, Subcommand};
use evt::EventArgs;
use model::ModelArgs;
use msg::MessageArgs;
use owo_colors::OwoColorize;
use pack::PacakgeArgs;
use proc::ProcArgs;
use serde::{Serialize, de::DeserializeOwned};
use snap::SnapshotArgs;
use task::TaskArgs;

#[derive(Debug, Parser)]
#[command(name = "act")]
#[command(multicall = true, styles=crate::util::CLAP_STYLING)]
pub struct ActsRootCommand {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "execute model commands")]
    Model(ModelArgs),
    #[command(about = "execute package commands")]
    Package(PacakgeArgs),
    #[command(about = "execute proc commands")]
    Proc(ProcArgs),
    #[command(about = "execute task commands")]
    Task(TaskArgs),
    #[command(about = "execute message commands")]
    Message(MessageArgs),
    #[command(about = "execute act commands")]
    Act(ActArgs),
    #[command(about = "execute event commands")]
    Event(EventArgs),
    #[command(about = "execute snapshot commands")]
    Snapshot(SnapshotArgs),
    #[command(about = "exit the cli")]
    Exit,
}

/// Parse one REPL line into the command it names.
///
/// The tokenizer and clap failures are the user's own words: they are returned
/// as an error carrying clap's diagnostic rather than printed and swallowed on
/// the way out.
pub fn parse(line: &str) -> anyhow::Result<ActsRootCommand> {
    let args = shlex::split(line).ok_or_else(|| anyhow::anyhow!("unbalanced quote in `{line}`"))?;
    // `ActsRootCommand` is a multicall command: clap reads the first token as
    // the applet name, which is exactly the subcommand the user typed, so the
    // line needs no synthetic argv[0].
    ActsRootCommand::try_parse_from(args).map_err(anyhow::Error::new)
}

/// The text of clap's `help`/`--version` answer, when the line asked for one
/// of those rather than naming a command. Answering a question is not a
/// failure, so the REPL prints this on stdout.
fn help_text(err: &anyhow::Error) -> Option<String> {
    use clap::error::ErrorKind;

    let clap = err.downcast_ref::<clap::Error>()?;
    matches!(
        clap.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    )
    .then(|| clap.to_string())
}

/// What the REPL prints for a line that failed.
///
/// clap's own rendering is authoritative for a parse failure — it names the
/// offending value and the usage — and is printed alone, because the chain
/// behind it (the value parser's error is clap's source) would repeat it.
/// Anything else prints with its whole chain, so a server's message is not
/// hidden behind the context that names the action.
pub fn error_text(err: &anyhow::Error) -> String {
    match err.downcast_ref::<clap::Error>() {
        Some(clap) => clap.to_string(),
        None => format!("{err:#}"),
    }
}

pub struct CommandRunner<'a> {
    vars: Vars,
    client: &'a mut ActsChannel,
}

impl<'a> CommandRunner<'a> {
    pub fn new(client: &'a mut ActsChannel) -> Self {
        Self {
            client,
            vars: Vars::new(),
        }
    }

    /// Send one action to the server. A failed round trip is reported as the
    /// action the user typed (`model:ls`, `proc:start`, …) plus what the
    /// server answered.
    pub async fn send<T>(&mut self, name: &str, vars: Vars) -> anyhow::Result<ActionResult<T>>
    where
        T: Serialize + DeserializeOwned,
    {
        self.client
            .send::<T>(name, vars)
            .await
            .map_err(|err| client::action_failed(name, err))
    }

    /// Deploy a workflow model (`model:deploy`).
    pub async fn deploy(&mut self, model: &str) -> anyhow::Result<ActionResult<bool>> {
        self.client
            .deploy(model, None)
            .await
            .map_err(|err| client::action_failed("model:deploy", err))
    }

    /// Publish a package (`pack:publish`).
    pub async fn publish(&mut self, package: &Package) -> anyhow::Result<ActionResult<bool>> {
        self.client
            .publish(package)
            .await
            .map_err(|err| client::action_failed("pack:publish", err))
    }

    /// Start a process of the model `mid` (`proc:start`).
    pub async fn start(&mut self, mid: &str, vars: Vars) -> anyhow::Result<ActionResult<String>> {
        self.client
            .start(mid, vars)
            .await
            .map_err(|err| client::action_failed("proc:start", err))
    }

    /// Ack a delivered message (`msg:ack`).
    pub async fn ack(&mut self, id: &str) -> anyhow::Result<ActionResult<()>> {
        self.client
            .ack(id)
            .await
            .map_err(|err| client::action_failed("msg:ack", err))
    }

    pub async fn run(&mut self, line: &str) -> anyhow::Result<bool> {
        let cli = match parse(line) {
            Ok(cli) => cli,
            Err(err) => {
                if let Some(text) = help_text(&err) {
                    println!("{text}");
                    return Ok(false);
                }
                return Err(err);
            }
        };
        match cli.command {
            Commands::Exit => {
                return Ok(true);
            }
            Commands::Model(args) => {
                model::process(self, &args.command).await?;
            }
            Commands::Package(args) => {
                pack::process(self, &args.command).await?;
            }
            Commands::Proc(args) => {
                proc::process(self, &args.command).await?;
            }
            Commands::Task(args) => {
                task::process(self, &args.command).await?;
            }
            Commands::Message(args) => {
                msg::process(self, &args.command).await?;
            }
            Commands::Act(args) => {
                act::process(self, &args.command).await?;
            }
            Commands::Event(args) => {
                evt::process(self, &args.command).await?;
            }
            Commands::Snapshot(args) => {
                snap::process(self, &args.command).await?;
            }
        };

        Ok(false)
    }

    pub fn output(&self, value: &str) {
        for line in value.lines() {
            println!("{}", line.green());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The REPL's whole input path: a line the user typed becomes the command
    /// it names, and a line that names nothing is an error carrying clap's
    /// diagnostic — printed help included.
    #[test]
    fn a_line_parses_into_the_command_it_names() {
        let cli = parse("model get approve").unwrap();
        assert!(matches!(cli.command, Commands::Model(_)));

        let cli = parse("snapshot upsert profile --scope u1 --rev 3 --data '{\"a\":1}'").unwrap();
        assert!(matches!(cli.command, Commands::Snapshot(_)));

        let Commands::Proc(args) = parse("proc start approve --pid p1").unwrap().command else {
            panic!("a proc line must parse into a proc command");
        };
        assert!(matches!(args.command, proc::ProcCommands::Start { .. }));

        assert!(matches!(parse("exit").unwrap().command, Commands::Exit));
    }

    /// A tokenizer that cannot split the line, and a line that names no
    /// command, are both reported — neither may panic the REPL.
    #[test]
    fn an_unparsable_line_is_an_error() {
        let err = parse("model get 'unterminated").unwrap_err();
        assert!(
            err.to_string().contains("unbalanced quote"),
            "unexpected error: {err}"
        );

        let err = parse("bogus").unwrap_err();
        assert!(
            err.to_string().contains("bogus"),
            "a clap diagnostic must name the rejected command: {err}"
        );

        let err = parse("model get").unwrap_err();
        assert!(err.to_string().contains("model"), "unexpected error: {err}");
    }

    /// `help` is a question clap answers, not a parse failure: the REPL prints
    /// its text instead of reporting it as an error.
    #[test]
    fn help_is_an_answer_not_a_failure() {
        let err = parse("help").unwrap_err();
        let text = help_text(&err).expect("clap's help must be recognized as help");
        assert!(text.contains("model"), "unexpected help text: {text}");

        // a real parse failure is not mistaken for help
        let err = parse("bogus").unwrap_err();
        assert!(help_text(&err).is_none());
    }

    /// A failed line is printed once: clap's rendering already carries the
    /// value parser's message, so the chain behind it (the parser's error is
    /// clap's source) is not appended to it. A failed action keeps its chain,
    /// which is what names the action and the server's message.
    #[test]
    fn a_failed_line_prints_one_diagnostic() {
        let err = parse("model ls -Q nope").unwrap_err();
        let text = error_text(&err);
        assert_eq!(
            text.matches("no `=` or `~` found").count(),
            1,
            "clap's rendering and the chain behind it must not both print: {text}"
        );

        let err = crate::client::action_failed("model:get", "code: 'Internal error'");
        assert_eq!(
            error_text(&err),
            "action 'model:get' failed: code: 'Internal error'"
        );
    }
}
