//! The CLI's command layer against a real server.
//!
//! Every case boots the shipped engine with the shipped gRPC plugin — the
//! server `acts-server` assembles — reached over gRPC, and then checks what a
//! command renders, because that text is the CLI's whole product. The second
//! half of each case is the point of the hardening this file guards: a server
//! that refuses (an unknown id, a denied action, an unreachable payload) must
//! come back as an error naming the action, never as the panic the `unwrap()`s
//! here used to be.

mod common;

use acts::UserSpec;
use acts_channel::Vars;
use acts_cli::{
    client,
    cmd::{CommandRunner, model, proc as proc_cmd, snap, task},
};
use anyhow::Context;
use serde_json::json;
use std::time::Duration;

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

/// The renderable half of a command's answer: everything before the elapsed
/// time the CLI appends.
fn body_of(out: &str) -> &str {
    out.split("(elapsed").next().unwrap_or(out)
}

/// Deploy a model, then read it back, list it, run it and remove it — the
/// whole model/proc/task/snapshot surface over one live server.
#[tokio::test(flavor = "multi_thread")]
async fn commands_render_the_answers_of_a_real_server() {
    common::with_server("commands", true, |_engine, url| async move {
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
    common::with_server("refusals", true, |_engine, url| async move {
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
/// startup greeting prints comes from the session a login answered with, what
/// the user's grants allow is served, and what they do not is an error naming
/// the action. A caller that never logged in reads the catalogue and nothing
/// else.
#[tokio::test(flavor = "multi_thread")]
async fn an_acl_refusal_names_the_action() {
    common::with_server("acl", false, |engine, url| async move {
        engine
            .acl()
            .set_user(&UserSpec {
                name: "reader".to_string(),
                add_passwords: vec!["s3cret".to_string()],
                allow: Some(vec!["model:ls".to_string()]),
                ..Default::default()
            })
            .await?;

        let mut channel =
            acts_channel::ActsChannel::connect_with_password(&url, "reader", "s3cret")
                .await
                .map_err(|err| client::action_failed("acl:login", err))?;
        let who = client::whoami(&mut channel).await?;
        assert_eq!(client::identity(&who), "reader");

        let mut cli = CommandRunner::new(&mut channel);
        assert!(
            model::ls(&mut cli, &None, &None, &vec![], &vec![])
                .await
                .is_ok(),
            "the user's grant allows model:ls"
        );

        let err = model::rm(&mut cli, "simple").await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("model:rm"), "unexpected error: {text}");
        assert!(
            text.contains("is not allowed for user 'reader'"),
            "a denied action must say so: {text}"
        );

        // without a session the server answers the catalogue reads, and a
        // missing credential is reported as such — not as a grant
        let mut anonymous = client::connect(&url, None).await?;
        let who = client::whoami(&mut anonymous).await?;
        assert_eq!(who["user"], json!("anonymous"));
        assert_eq!(who["authenticated"], json!(false));
        assert_eq!(
            client::identity(&who),
            "anonymous",
            "an unauthenticated session greets as anonymous"
        );
        assert!(
            anonymous
                .send::<serde_json::Value>("model:ls", Vars::new())
                .await
                .is_ok(),
            "an anonymous caller reads the catalogue"
        );
        // over the CLI surface the action is named, and what is missing is the
        // credential — not a grant
        let mut anonymous_cli = CommandRunner::new(&mut anonymous);
        let err = anonymous_cli.run("auth user ls").await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("acl:users"), "unexpected error: {text}");
        assert!(
            text.contains("a session token is required"),
            "a missing credential must be reported as one: {text}"
        );

        Ok(())
    })
    .await
    .expect("the acl refusals must be reported as errors");
}

/// The `auth` command group against the user registry: an administrator
/// declares, reads and deletes users; an ordinary user is refused by the
/// server, and the builtin admin cannot be deleted out from under it.
#[tokio::test(flavor = "multi_thread")]
async fn the_auth_commands_manage_the_engines_users() {
    common::with_server("users", false, |engine, url| async move {
        // the builtin admin's password is whatever this case gives it
        engine
            .acl()
            .set_user(&UserSpec {
                name: "admin".to_string(),
                add_passwords: vec!["root".to_string()],
                ..Default::default()
            })
            .await?;

        let mut channel = acts_channel::ActsChannel::connect_with_password(&url, "admin", "root")
            .await
            .map_err(|err| client::action_failed("acl:login", err))?;
        let mut cli = CommandRunner::new(&mut channel);

        // `auth user set` is what declares a user now
        cli.run("auth user set reader --allow @read --password s3cret")
            .await?;
        let reader = engine
            .acl()
            .get_user("reader")
            .await
            .expect("the user the command saved is readable");
        assert_eq!(reader["name"], json!("reader"));
        assert_eq!(reader["allow"], json!(["@read"]));

        // `auth user ls` and `get` read the registry back
        cli.run("auth user ls").await?;
        assert!(
            engine
                .acl()
                .user_names()
                .await
                .contains(&"reader".to_string())
        );
        cli.run("auth user get reader").await?;

        // the builtin administrator is not the CLI's to remove
        let err = cli.run("auth user rm admin").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("cannot be deleted"),
            "unexpected error: {err:#}"
        );

        // the registry's reads belong to `@read`, so this user may list and
        // read users — changing them is `@write`, and the server refuses that
        let mut reader = acts_channel::ActsChannel::connect_with_password(&url, "reader", "s3cret")
            .await
            .map_err(|err| client::action_failed("acl:login", err))?;
        let mut reader_cli = CommandRunner::new(&mut reader);
        reader_cli.run("auth user ls").await?;
        reader_cli.run("auth user get reader").await?;

        for line in ["auth user set other --password x", "auth user rm admin"] {
            let err = reader_cli.run(line).await.unwrap_err();
            let text = format!("{err:#}");
            assert!(
                text.contains("is not allowed for user 'reader'"),
                "`{line}` must be refused: {text}"
            );
        }

        // `auth user rm` deletes the user, and its login is refused from then on
        cli.run("auth user rm reader").await?;
        assert!(engine.acl().get_user("reader").await.is_none());
        let err = acts_channel::ActsChannel::connect_with_password(&url, "reader", "s3cret")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid user or password"),
            "unexpected error: {err}"
        );

        Ok(())
    })
    .await
    .expect("the user commands must answer");
}

/// The REPL's own paths: a command runs, `help` answers instead of failing,
/// `exit` ends the session, and a line that names nothing is an error.
#[tokio::test(flavor = "multi_thread")]
async fn the_repl_reads_a_line_without_panicking() {
    common::with_server("repl", true, |_engine, url| async move {
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
