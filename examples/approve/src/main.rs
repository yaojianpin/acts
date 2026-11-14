//! The approve demo: deploy the workflow and drive its human acts.
//!
//! Every act the engine announces waits for a decision. The demo answers in
//! one of three ways: an interactive arrow-key menu, a script of recorded
//! choices (`--script`, one line per prompt — a select's line is the item
//! text, its 1-based position, or a stable key, a blank line takes the
//! default), or the menu plus a recording (`--record` appends one line per
//! answer, in the exact format `--script` consumes, so a misbehaving session
//! hands over its own repro). The business system's own answers (candidate
//! pools, delegated join verdicts) live in `sys.rs`; the menus live in
//! `client.rs`. The run starts with the amount the demo asks for first — the
//! **first line of a script** (blank keeps the model's default 800) — the one
//! input that decides how far the expense travels.
//!
//! Exit codes: `0` the run completed (or a scripted session's choices ran
//! out and the demo stopped), `2` it ended through its error walk (an
//! `error`/`abort` decision), `1` a setup failure (model, script, IO).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use acts::{Engine, Result, Vars, Workflow};
use clap::Parser;
use serde_json::json;

mod client;
mod sys;

/// Deploy the approve model and decide its acts — from the terminal, from a
/// script of recorded choices, or from the menu while recording it.
#[derive(Parser, Debug)]
#[command(name = "approve", version, about)]
struct Cli {
    /// Drive the menu from a decisions script, one choice per line
    /// (`-` reads the script from stdin).
    #[arg(short, long)]
    script: Option<String>,
    /// Record every menu answer into this file, one per line — the format
    /// `--script` consumes, so an interactive session becomes its own repro
    /// (ignored together with `--script`).
    #[arg(long, value_name = "FILE")]
    record: Option<std::path::PathBuf>,
    /// Read the workflow model from this YAML file instead of the embedded
    /// `model.yml`.
    #[arg(short, long)]
    model: Option<std::path::PathBuf>,
    /// Enable kernel logs with this filter (for example `acts=debug`).
    #[arg(short, long)]
    log: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(filter) = &cli.log {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_target(false)
            .try_init();
    }

    let engine = Engine::builder().start().await?;
    let (s, sig) = engine.signal(()).double();
    let failed = Arc::new(AtomicBool::new(false));

    // The business system answers what needs no user (the candidate pools,
    // the delegated join verdicts); the client puts every human act to the
    // menu.
    let sys = sys::Sys::new();
    // A script that runs out of answers leaves the rest to the engine; the
    // demo stops waiting instead of parking forever on an act no one will
    // decide.
    let stop = Arc::new(tokio::sync::Notify::new());
    let mut client = client::Client::new().stop_when_exhausted(stop.clone());
    if let Some(script) = &cli.script {
        client = client.script(script).unwrap_or_else(|error| {
            eprintln!("cannot read the script '{script}': {error}");
            std::process::exit(1);
        });
    } else if let Some(record) = &cli.record {
        client = client.recording(record).unwrap_or_else(|error| {
            eprintln!("cannot write the recording '{}': {error}", record.display());
            std::process::exit(1);
        });
    }
    client.bind(&engine, &sys.users());
    let sys = Arc::new(sys);
    let client = Arc::new(client);

    let text = match &cli.model {
        Some(path) => std::fs::read_to_string(path).unwrap_or_else(|error| {
            eprintln!("cannot read the model '{}': {error}", path.display());
            std::process::exit(1);
        }),
        None => include_str!("./model.yml").to_string(),
    };
    let workflow = Workflow::from_yml(&text).unwrap_or_else(|error| {
        eprintln!("the model does not parse: {error}");
        std::process::exit(1);
    });
    print!("{}", workflow.tree_output());

    let executor = engine.executor(&acts::Principal::unrestricted()).clone();
    executor.model().deploy(&workflow, None).await?;
    let message_executor = executor.clone();
    let complete_executor = executor.clone();
    let error_executor = executor.clone();
    let complete_signal = s.clone();
    let error_signal = s.clone();
    let complete_failed = failed.clone();
    let client_for_message = client.clone();
    engine.channel().on_message(move |e| {
        let sys = sys.clone();
        let client = client_for_message.clone();
        let executor = message_executor.clone();
        async move {
            if let Err(err) = sys.process(&executor, &e).await {
                eprintln!("{err}");
                std::process::exit(1);
            }
            if let Err(err) = client.process(&executor, &e).await {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
    });
    engine.channel().on_complete(move |e| {
        let s = complete_signal.clone();
        let complete_executor = complete_executor.clone();
        let complete_flag = complete_failed.clone();
        async move {
            println!(
                "on_workflow_complete: pid={} state={} cost={}ms outputs={:?}",
                e.pid,
                e.state(),
                e.cost(),
                e.outputs
            );
            match complete_executor.proc().graph(&e.pid).await {
                Ok(graph) => println!("{}", graph.tree_output()),
                Err(err) => eprintln!("cannot read the finished run's graph: {err}"),
            }
            // An aborted or cancelled run closes through this event too: it
            // is a settlement, not a success — exit with code 2 like the
            // error walk's own event.
            if !matches!(e.state(), acts::MessageState::Completed) {
                complete_flag.store(true, Ordering::SeqCst);
            }
            s.close();
        }
    });
    // A `skip`ped step moves on, but an `error`/`abort` ends the run through
    // its error walk: report it and exit with code 2.
    let error_flag = failed.clone();
    engine.channel().on_error(move |e| {
        let s = error_signal.clone();
        let error_executor = error_executor.clone();
        let error_flag = error_flag.clone();
        async move {
            println!(
                "on_workflow_error: pid={} state={} cost={}ms outputs={:?}",
                e.pid,
                e.state(),
                e.cost(),
                e.outputs
            );
            match error_executor.proc().graph(&e.pid).await {
                Ok(graph) => println!("{}", graph.tree_output()),
                Err(err) => eprintln!("cannot read the failed run's graph: {err}"),
            }
            error_flag.store(true, Ordering::SeqCst);
            s.close();
        }
    });

    executor
        .proc()
        .start(
            &workflow.id,
            Vars::new()
                .with("actor", "101")
                .with("amount", json!(client.ask_amount().await)),
        )
        .await?;
    tokio::select! {
        _ = sig.recv() => {}
        _ = stop.notified() => {
            println!(
                "the script ran out of choices — the run is left incomplete; \
                 extend the script to decide the remaining acts"
            );
        }
    }

    if failed.load(Ordering::SeqCst) {
        std::process::exit(2);
    }
    Ok(())
}
