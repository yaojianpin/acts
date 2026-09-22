//! The CLI's command layer against a real server.
//!
//! Every case boots the shipped engine with the shipped gRPC plugin — the
//! server `acts-server` assembles — reached over gRPC, and then checks what a
//! command renders, because that text is the CLI's whole product. The second
//! half of each case is the point of the hardening this file guards: a server
//! that refuses (an unknown id, a denied action, an unreachable payload) must
//! come back as an error naming the action, never as the panic the `unwrap()`s
//! here used to be.

use acts::{Config, Engine};
use acts_channel::Vars;
use acts_cli::{
    client,
    cmd::{CommandRunner, model, proc as proc_cmd, snap, task},
};
use acts_plugin_grpc::GrpcPlugin;
use anyhow::Context;
use serde_json::json;
use std::{
    future::Future,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// A model whose step waits for a client act (`acts.core.irq`): the run stays
/// live, so its process and task rows are there to read — a run that finishes
/// is swept with its rows, and the sweep would race the assertions.
const MODEL: &str = r#"
name: simple
id: simple
ver: 0.1.0
steps:
  - name: init
    id: init
    uses: acts.core.irq
    params:
      key: cli-test
"#;

/// Boot a server for one case and hand its endpoint to `body`.
///
/// `config_extra` is the config the server runs on — the `[acl]` section of
/// the case — and the gRPC port is picked per test, so two cases run in
/// parallel.
async fn with_server<F, Fut>(name: &str, config_extra: &str, body: F) -> anyhow::Result<()>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    // The port is picked before the server can bind it, so a collision (the
    // same ephemeral port handed to a sibling case right after it released it)
    // is retried on a fresh port instead of failing on a listener that never
    // came up. A case that fails for its own reason still fails once — the
    // retry is only for the port.
    let mut last = None;
    for _ in 0..3 {
        let dir = scratch(name)?;
        let port = free_port()?;
        let config_path = dir.join("acts.toml");
        std::fs::write(
            &config_path,
            format!("{config_extra}\n[grpc]\nport = {port}\n"),
        )
        .with_context(|| format!("failed to write {}", config_path.display()))?;

        // the engine and the transport plugin are the shipped ones — a CLI test
        // must not be checking a second, hand-rolled wiring that can drift
        let config = Config::create(&config_path)
            .with_context(|| format!("failed to load {}", config_path.display()))?;
        let engine = Engine::builder()
            .set_config(&config)
            .add_plugin(&GrpcPlugin::new())
            .start()
            .await
            .context("the test server must start")?;

        let url = format!("http://127.0.0.1:{port}");
        if let Err(err) = wait_for_server(&url).await {
            engine.close().await;
            let _ = std::fs::remove_dir_all(&dir);
            last = Some(err);
            continue;
        }

        let result = body(url).await;
        engine.close().await;
        let _ = std::fs::remove_dir_all(&dir);
        return result;
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("the server never accepted a connection")))
}

/// How long a case waits for the server to accept a connection. The gRPC
/// listener binds in a spawned task, and CI runs this suite instrumented
/// (`cargo llvm-cov`) on a shared runner, so the budget is generous: waiting
/// it out means something is actually wrong.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

async fn wait_for_server(url: &str) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        match client::connect(url, None).await {
            Ok(_) => return Ok(()),
            Err(err) => last = err.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err(anyhow::anyhow!(
        "{url} never accepted a connection within {READY_TIMEOUT:?}: {last}"
    ))
}

fn scratch(name: &str) -> anyhow::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("the clock is before the epoch: {err}"))?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("acts-cli-{}-{name}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

fn free_port() -> anyhow::Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("failed to bind a free port")?;
    let port = listener
        .local_addr()
        .context("failed to read the bound address")?
        .port();
    drop(listener);
    Ok(port)
}

/// The renderable half of a command's answer: everything before the elapsed
/// time the CLI appends.
fn body_of(out: &str) -> &str {
    out.split("(elapsed").next().unwrap_or(out)
}

/// Deploy a model, then read it back, list it, run it and remove it — the
/// whole model/proc/task/snapshot surface over one live server.
#[tokio::test(flavor = "multi_thread")]
async fn commands_render_the_answers_of_a_real_server() {
    with_server("commands", "[acl]\nenabled = false\n", |url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        // an empty server answers an empty page
        let out = model::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert!(out.contains("total 0"), "unexpected page: {out}");

        let model_file = std::env::temp_dir().join(format!("acts-cli-{}.yml", std::process::id()));
        std::fs::write(&model_file, MODEL).context("failed to write the model file")?;
        model::deploy(&mut cli, &model_file).await?;
        let _ = std::fs::remove_file(&model_file);

        let out = model::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert!(out.contains("total 1"), "unexpected page: {out}");

        // `model get` answers the model the server holds
        let out = model::get(&mut cli, "simple", &None).await?;
        assert!(out.contains("simple"), "unexpected model: {out}");

        // start a process: the pid the CLI prints is the one that was asked for
        let out =
            proc_cmd::start(&mut cli, "simple", &Some("p1".to_string()), &Vars::new()).await?;
        assert!(out.starts_with("pid=p1"), "unexpected start answer: {out}");
        let out = proc_cmd::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert!(out.contains("total 1"), "unexpected page: {out}");

        let out = proc_cmd::get(&mut cli, "p1", &None).await?;
        let proc: serde_json::Value =
            serde_json::from_str(body_of(&out)).context("failed to parse the proc answer")?;
        assert_eq!(proc["id"], "p1");
        assert_eq!(proc["mid"], "simple");

        // the step's task row is written as the run is scheduled: wait for it
        let mut last = String::new();
        for _ in 0..600 {
            last = task::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
            if !last.starts_with("total 0") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !last.starts_with("total 0"),
            "the task never showed up: {last}"
        );

        let out = proc_cmd::get(&mut cli, "p1", &None).await?;
        let proc: serde_json::Value = serde_json::from_str(body_of(&out))?;
        let tid = proc["tasks"][0]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!tid.is_empty(), "the process answered no task: {out}");
        let out = task::get(&mut cli, "p1", &tid).await?;
        assert!(out.contains(&tid), "unexpected task: {out}");

        // snapshot: an unregistered target is created by the first upsert
        let out = snap::upsert(&mut cli, "profile", "u1", 3, &json!({"rate_limit": 100})).await?;
        assert!(out.contains("updated (rev 3)"), "unexpected upsert: {out}");

        let out = snap::get(&mut cli, "profile", "u1").await?;
        assert!(
            out.contains("rate_limit: 100"),
            "unexpected snapshot: {out}"
        );

        let out = snap::ls(&mut cli, "profile").await?;
        assert!(
            !out.contains("no snapshot data"),
            "a written scope must be listed: {out}"
        );

        let out = snap::remove(&mut cli, "profile", "u1").await?;
        assert!(out.contains("removed"), "unexpected remove: {out}");
        let out = snap::get(&mut cli, "profile", "u1").await?;
        assert!(
            out.contains("snapshot not found"),
            "unexpected snapshot: {out}"
        );

        let out = snap::ls(&mut cli, "nothing").await?;
        assert!(
            out.contains("no snapshot data"),
            "an empty target must say so: {out}"
        );

        // a settled model is gone once it is removed
        model::rm(&mut cli, "simple").await?;
        let err = model::get(&mut cli, "simple", &None).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("model:get"),
            "a refusal must name the action: {err:#}"
        );

        Ok(())
    })
    .await
    .expect("the model, process and snapshot commands must all answer");
}

/// What a server refuses is an error naming the action and the server's own
/// message — the answer the REPL prints instead of dying on a panic.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_names_the_action_and_the_message() {
    with_server("refusals", "[acl]\nenabled = false\n", |url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        let err = model::get(&mut cli, "missing", &None).await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("model:get"), "unexpected error: {text}");
        assert!(text.contains("missing"), "unexpected error: {text}");

        let err = proc_cmd::get(&mut cli, "missing", &None).await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("proc:get"), "unexpected error: {text}");

        let err = task::get(&mut cli, "missing", "missing").await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("task:get"), "unexpected error: {text}");

        // a snapshot remove never registers its target, unlike an upsert
        let err = snap::remove(&mut cli, "nothing", "u1").await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("snap:remove"), "unexpected error: {text}");
        assert!(
            text.contains("is not registered"),
            "unexpected error: {text}"
        );

        Ok(())
    })
    .await
    .expect("every refusal must be reported as an error");
}

/// An ACL refusal is the transport's, not the engine's: the identity the
/// startup greeting prints comes from the token, what the role allows is
/// served, and what it does not is an error naming the action.
#[tokio::test(flavor = "multi_thread")]
async fn an_acl_refusal_names_the_action() {
    let acl = r#"
[acl]
[[acl.role]]
name = "reader"
tokens = ["reader-token"]
allow = ["model:ls"]
"#;
    with_server("acl", acl, |url| async move {
        let mut channel = client::connect(&url, Some("reader-token".to_string())).await?;
        let who = client::whoami(&mut channel).await?;
        assert_eq!(client::identity(&who), "roles: reader");

        let mut cli = CommandRunner::new(&mut channel);
        assert!(
            model::ls(&mut cli, &None, &None, &vec![], &vec![])
                .await
                .is_ok(),
            "the role allows model:ls"
        );

        let err = model::rm(&mut cli, "simple").await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("model:rm"), "unexpected error: {text}");
        assert!(
            text.contains("is not allowed for role(s) reader"),
            "a denied action must say so: {text}"
        );

        // without a token the server refuses the startup check itself
        let mut anonymous = client::connect(&url, None).await?;
        let err = client::whoami(&mut anonymous).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("acl:whoami"),
            "unexpected error: {err:#}"
        );

        Ok(())
    })
    .await
    .expect("the acl refusals must be reported as errors");
}

/// The REPL's own paths: a command runs, `help` answers instead of failing,
/// `exit` ends the session, and a line that names nothing is an error.
#[tokio::test(flavor = "multi_thread")]
async fn the_repl_reads_a_line_without_panicking() {
    with_server("repl", "[acl]\nenabled = false\n", |url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        assert!(
            !cli.run("model ls").await?,
            "a command must not end the session"
        );
        assert!(
            cli.run("help").await.is_ok(),
            "help is an answer, not a failure"
        );
        assert!(cli.run("exit").await?, "exit must end the session");

        let err = cli.run("bogus").await.unwrap_err();
        assert!(
            err.to_string().contains("bogus"),
            "a rejected command must be named: {err}"
        );

        let err = cli.run("model get 'unterminated").await.unwrap_err();
        assert!(
            err.to_string().contains("unbalanced quote"),
            "unexpected error: {err}"
        );

        Ok(())
    })
    .await
    .expect("the REPL must read every line without panicking");
}
