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
use acts_channel::{
    Vars,
    model::{MessageInfo, PageData, Query},
};
use acts_cli::{
    client,
    cmd::{CommandRunner, evt, model, msg, pack, proc as proc_cmd, snap, status, task},
    util,
};
use anyhow::Context;
use serde_json::json;
use std::{
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
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

/// A graph with a waiting step, a plain step over a branch, and a step behind
/// it: the branches and the flow ahead are what `proc graph` renders, and the
/// vars/params are what its filters read.
const GRAPH_MODEL: &str = r#"
name: Graph Cli
id: graph-cli
ver: 0.1.0
steps:
  - name: wait
    id: wait
    uses: acts.core.irq
    params:
      key: hold
    vars:
      - name: token
        value: abc
  - name: outer
    id: outer
    branches:
      - name: branch1
        id: branch1
        if: 1 > 0
        vars:
          - name: stage
            value: review
        steps:
          - name: notify
            id: notify
            uses: acts.core.msg
            params:
              key: msg1
            vars:
              - name: count
                value: 2
  - name: tail
    id: tail
    uses: acts.core.irq
    params:
      key: tail
"#;

/// `status` answers the server's vitals over the wire: every section the
/// engine reports gets its line, on a real server reached the way the REPL
/// reaches it.
#[tokio::test(flavor = "multi_thread")]
async fn the_status_command_answers_the_server_vitals() {
    common::with_server("status", true, |_engine, url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        let out = status::show(&mut cli).await?;
        let body = body_of(&out);
        assert!(body.contains("version      "), "unexpected status: {body}");
        assert!(
            body.contains("health       healthy"),
            "unexpected status: {body}"
        );
        assert!(
            body.contains("processes    total "),
            "unexpected status: {body}"
        );
        assert!(
            body.contains("memory       rss "),
            "unexpected status: {body}"
        );
        assert!(
            body.contains("cpu          process "),
            "unexpected status: {body}"
        );
        assert!(body.contains("storage      "), "unexpected status: {body}");
        assert!(
            body.contains("  loop       trigger "),
            "unexpected status: {body}"
        );
        assert!(body.contains("config       "), "unexpected status: {body}");
        Ok(())
    })
    .await
    .expect("the status command must answer with the server's vitals");
}

/// The renderable half of a command's answer: everything before the elapsed
/// time the CLI appends.
fn body_of(out: &str) -> &str {
    out.split("(elapsed").next().unwrap_or(out)
}

/// The `total N, page …` count a listing answers with.
fn page_total(out: &str) -> usize {
    out.strip_prefix("total ")
        .and_then(|rest| rest.split(',').next())
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or_default()
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

/// `proc graph` renders the run's declared tree with its live states, appends
/// the step vars the user asks for, and applies `--query` inclusions and
/// `--exclude-query` vetoes (including the reserved `uses` key) — a hidden
/// step takes its subtree with it.
#[tokio::test(flavor = "multi_thread")]
async fn the_proc_graph_command_renders_vars_and_filters_steps() {
    common::with_server("graph", true, |_engine, url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        let file = scratch_file("graph", GRAPH_MODEL);
        model::deploy(&mut cli, &file).await?;
        let _ = std::fs::remove_file(&file);
        proc_cmd::start(&mut cli, "graph-cli", &Some("g1".to_string()), &Vars::new()).await?;
        let _ = wait_task_id(&mut cli, "g1").await;

        // The whole declaration, in declaration order, with the flow behind
        // the waiting act still stateless. No vars unless they are asked for.
        let out = proc_cmd::graph(&mut cli, "g1", &[], &[], &[]).await?;
        let body = body_of(&out);
        assert!(
            body.contains("workflow id:$ name=Graph Cli"),
            "the root must lead: {body}"
        );
        assert!(
            body.contains("step id:wait name=wait  uses=acts.core.irq"),
            "the waiting step must be there: {body}"
        );
        assert!(
            body.contains("step id:outer name=outer"),
            "the container step must be there: {body}"
        );
        assert!(
            body.contains("branch id:branch1 name=branch1"),
            "the declared branch must be there: {body}"
        );
        assert!(
            body.contains("step id:notify name=notify  uses=acts.core.msg state=nil"),
            "the unreached act must carry no state: {body}"
        );
        assert!(
            body.contains("step id:tail name=tail  uses=acts.core.irq state=nil"),
            "the tail is ahead of the waiting act: {body}"
        );
        assert!(!body.contains("vars="), "no var was requested: {body}");

        // --var picks the named declaration; a name the node does not declare
        // adds nothing.
        let out = proc_cmd::graph(&mut cli, "g1", &["token".to_string()], &[], &[]).await?;
        assert!(
            out.contains(r#"vars={"token":"abc"}"#),
            "the requested var must be on its step: {out}"
        );
        assert!(
            !out.contains("count") && !out.contains("stage"),
            "an unrequested var must stay hidden: {out}"
        );
        let out = proc_cmd::graph(&mut cli, "g1", &["stage".to_string()], &[], &[]).await?;
        assert!(
            out.contains(r#"vars={"stage":"review"}"#),
            "a branch var must show on its branch: {out}"
        );
        let out = proc_cmd::graph(&mut cli, "g1", &["missing".to_string()], &[], &[]).await?;
        assert!(!out.contains("vars="), "an absent var adds no field: {out}");

        // A bare --var (`*`) shows every declared var.
        let out = proc_cmd::graph(&mut cli, "g1", &["*".to_string()], &[], &[]).await?;
        assert!(
            out.contains(r#"vars={"token":"abc"}"#),
            "all vars must show on the waiting step: {out}"
        );
        assert!(
            out.contains(r#"vars={"count":2}"#),
            "all vars must show on the branched step: {out}"
        );
        assert!(
            out.contains(r#"vars={"stage":"review"}"#),
            "all vars must show on the branch itself: {out}"
        );

        // `--query uses=...` keeps only the irq steps; the plain step over
        // the branch does not match, so its subtree goes with it.
        let out = proc_cmd::graph(
            &mut cli,
            "g1",
            &[],
            &[("uses".to_string(), Some(json!("acts.core.irq")))],
            &[],
        )
        .await?;
        assert!(out.contains("id:wait"), "a matching step stays: {out}");
        assert!(out.contains("id:tail"), "a matching step stays: {out}");
        assert!(!out.contains("id:outer"), "a non-matching step goes: {out}");
        assert!(
            !out.contains("id:notify"),
            "a hidden step's subtree goes with it: {out}"
        );

        // `--query key=...` requires the declared pair; a paramless step (and
        // the branch under it) has nothing to match.
        let out = proc_cmd::graph(
            &mut cli,
            "g1",
            &[],
            &[("key".to_string(), Some(json!("hold")))],
            &[],
        )
        .await?;
        assert!(out.contains("id:wait"), "the matching step stays: {out}");
        assert!(!out.contains("id:tail"), "another value is filtered: {out}");
        assert!(
            !out.contains("id:outer"),
            "a paramless step is filtered: {out}"
        );

        // A bare key includes on presence: the top-level steps declaring
        // `key` stay. `notify` declares it too, but its parent `outer` does
        // not match, so the branch subtree is filtered with it.
        let out = proc_cmd::graph(&mut cli, "g1", &[], &[("key".to_string(), None)], &[]).await?;
        assert!(out.contains("id:wait"), "a declared key matches: {out}");
        assert!(out.contains("id:tail"), "a declared key matches: {out}");
        assert!(
            !out.contains("id:notify"),
            "a hidden parent takes its subtree: {out}"
        );
        assert!(
            !out.contains("id:outer"),
            "a paramless step does not match: {out}"
        );

        // `--exclude-query uses=...` vetoes both irq steps; a step with
        // another `uses` (or no `uses` at all) keeps showing.
        let out = proc_cmd::graph(
            &mut cli,
            "g1",
            &[],
            &[],
            &[("uses".to_string(), Some(json!("acts.core.irq")))],
        )
        .await?;
        assert!(!out.contains("id:wait"), "an excluded step goes: {out}");
        assert!(!out.contains("id:tail"), "an excluded step goes: {out}");
        assert!(
            out.contains("id:outer"),
            "a step with no uses is not excluded: {out}"
        );
        assert!(
            out.contains("id:notify"),
            "a step with another uses is not excluded: {out}"
        );

        // --exclude-query is a veto: only the step carrying the pair goes,
        // and everything else keeps showing — a paramless step included.
        let out = proc_cmd::graph(
            &mut cli,
            "g1",
            &[],
            &[],
            &[("key".to_string(), Some(json!("hold")))],
        )
        .await?;
        assert!(!out.contains("id:wait"), "the excluded step goes: {out}");
        assert!(
            out.contains("id:outer"),
            "a paramless step is not excluded: {out}"
        );
        assert!(
            out.contains("id:notify"),
            "a step with another value is not excluded: {out}"
        );
        assert!(
            out.contains("id:tail"),
            "a step with another value is not excluded: {out}"
        );

        // A bare key vetoes on presence, so every step declaring `key` goes;
        // the plain step over the branch stays.
        let out = proc_cmd::graph(&mut cli, "g1", &[], &[], &[("key".to_string(), None)]).await?;
        assert!(out.contains("id:outer"), "the plain step stays: {out}");
        assert!(
            !out.contains("id:wait"),
            "a declared key is excluded: {out}"
        );
        assert!(
            !out.contains("id:notify"),
            "a declared key is excluded: {out}"
        );
        assert!(
            !out.contains("id:tail"),
            "a declared key is excluded: {out}"
        );
        assert!(
            !out.contains("id:branch1"),
            "a branch with no visible descendant goes: {out}"
        );

        // The two directions compose: `uses=...` admits both irq steps, the
        // `key=tail` veto drops `tail`.
        let out = proc_cmd::graph(
            &mut cli,
            "g1",
            &[],
            &[("uses".to_string(), Some(json!("acts.core.irq")))],
            &[("key".to_string(), Some(json!("tail")))],
        )
        .await?;
        assert!(out.contains("id:wait"), "an admitted step stays: {out}");
        assert!(!out.contains("id:tail"), "the veto still wins: {out}");

        Ok(())
    })
    .await
    .expect("proc graph must render the graph, its vars and its filtered steps");
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

        // every list a user may carry is an edit of that list, and the
        // enable/disable flags are mutually exclusive
        cli.run(
            "auth user set editor --allow @read --deny 'model:rm' --pattern 'orders:*' \
             --snapshot 'secrets=$subject,team=*' --password s3cret --rm-password old --disable",
        )
        .await?;
        let editor = engine
            .acl()
            .get_user("editor")
            .await
            .expect("the user the command saved is readable");
        assert_eq!(editor["deny"], json!(["model:rm"]));
        assert_eq!(editor["patterns"], json!(["orders:*"]));
        assert_eq!(editor["enabled"], json!(false));
        assert!(
            editor["snapshot"].to_string().contains("secrets"),
            "the snapshot grants must be saved: {editor}"
        );
        cli.run("auth user set editor --enable").await?;
        assert_eq!(
            engine.acl().get_user("editor").await.expect("still there")["enabled"],
            json!(true)
        );
        let err = cli
            .run("auth user set editor --disable --enable")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("mutually exclusive"),
            "unexpected error: {err}"
        );

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

/// A model with two irq steps: the first waits for a client act while the
/// second is still ahead of it, which is the forward work `act cancel` rewinds
/// (a lone step has nothing to cancel).
const ACT_MODEL: &str = r#"
name: acts
id: acts
ver: 0.1.0
steps:
  - name: first
    id: first
    uses: acts.core.irq
    params:
      key: cli-act-one
  - name: second
    id: second
    uses: acts.core.irq
    params:
      key: cli-act-two
"#;

/// A model that exposes one manual trigger (`fired`) and one irq step: firing
/// the trigger starts a run that stays live, which is what the process and
/// task rows read back.
const TRIGGER_MODEL: &str = r#"
name: triggered
id: triggered
ver: 0.1.0
on:
  - id: fired
    name: fired
    kind: manual
steps:
  - name: init
    id: init
    uses: acts.core.irq
    params:
      key: cli-evt
"#;

/// A package definition the published catalogue can hold: every field the
/// `Package` wire shape requires.
const PACKAGE: &str = r#"
id: cli.package
name: cli package
desc: a package published by a test
icon: icon-cli
doc: https://example.test/cli
version: 0.1.0
schema: {}
options: null
run_as: func
resources: []
catalog: app
"#;

static FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write a scratch file for one case, named after the case and unique within
/// the test process.
fn scratch_file(name: &str, text: &str) -> PathBuf {
    let seq = FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("acts-cli-{}-{name}-{seq}", std::process::id()));
    std::fs::write(&path, text).expect("the scratch file must be writable");
    path
}

/// A scratch config directory, so a spawned CLI never reads or writes the
/// developer's own session.
fn scratch_dir(name: &str) -> PathBuf {
    let seq = FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("acts-cli-{}-{name}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the scratch directory must be creatable");
    dir
}

/// Run the built binary with `args`, feeding `input` to its stdin.
fn run_binary(args: &[&str], input: Option<&str>, config: &Path) -> std::process::Output {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_acts-cli"))
        .args(args)
        .env("ACTS_CONFIG_DIR", config)
        .env_remove("ACTS_TOKEN")
        .env_remove("ACTS_USER")
        .env_remove("ACTS_PASSWORD")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the CLI binary must start");
    if let Some(input) = input {
        child
            .stdin
            .as_mut()
            .expect("stdin is piped")
            .write_all(input.as_bytes())
            .expect("the input must be written");
    }
    // closing stdin is what lets the REPL see the end of its input
    drop(child.stdin.take());
    child.wait_with_output().expect("the CLI binary must exit")
}

/// The first scheduled task of `pid`: the row appears as the run is
/// scheduled, so the wait is bounded rather than a fixed sleep.
async fn wait_task_id(cli: &mut CommandRunner<'_>, pid: &str) -> String {
    for _ in 0..600 {
        if let Ok(out) = proc_cmd::get(cli, pid, &None).await
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(body_of(&out))
        {
            // Ready/Running both exist while the run schedules; a client
            // action is accepted once the task is past Ready. (The irq act
            // row itself stays `running` while it waits for the client.)
            let task = value["tasks"].as_array().and_then(|tasks| {
                tasks.iter().find(|task| {
                    matches!(
                        task["state"].as_str().unwrap_or_default(),
                        "running" | "waiting"
                    )
                })
            });
            if let Some(tid) = task.and_then(|task| task["id"].as_str()) {
                return tid.to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the task of process '{pid}' never appeared");
}

/// The package catalogue: publish reads a definition, get prints it back as
/// yaml, ls pages it, and rm removes it — plus the file errors that never
/// reach the server.
#[tokio::test(flavor = "multi_thread")]
async fn the_package_commands_publish_read_and_remove() {
    common::with_server("packages", true, |_engine, url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        // the server ships its builtin packages; the published one adds to them
        let out = pack::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        let builtin = page_total(&out);
        // the REPL spelling reaches the same command
        cli.run("package ls").await?;

        let file = scratch_file("package", PACKAGE);
        let out = pack::publish(&mut cli, &file).await?;
        assert!(out.contains("true"), "unexpected publish: {out}");
        let _ = std::fs::remove_file(&file);

        let out = pack::get(&mut cli, "cli.package").await?;
        assert!(out.contains("cli.package"), "unexpected package: {out}");
        assert!(
            out.contains("a package published by a test"),
            "unexpected package: {out}"
        );

        let out = pack::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert_eq!(page_total(&out), builtin + 1, "unexpected page: {out}");
        let out = pack::ls(
            &mut cli,
            &Some(0),
            &Some(10),
            &vec![util::parse_key_value("id~cli").unwrap()],
            &vec![util::parse_sort("create_time!").unwrap()],
        )
        .await?;
        assert!(out.contains("total 1"), "unexpected page: {out}");

        let out = pack::rm(&mut cli, "cli.package").await?;
        assert!(out.contains("(elapsed"), "unexpected remove: {out}");
        let err = pack::get(&mut cli, "cli.package").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("pack:get"),
            "a refusal must name the action: {err:#}"
        );

        // a missing file and a malformed definition are reported before the
        // server is asked anything
        let missing = std::env::temp_dir().join("acts-cli-no-such-package.yml");
        let err = pack::publish(&mut cli, &missing).await.unwrap_err();
        assert!(
            err.to_string().contains("failed to read the package file"),
            "unexpected error: {err}"
        );
        let malformed = scratch_file("bad-package", "id: [unbalanced");
        let err = pack::publish(&mut cli, &malformed).await.unwrap_err();
        assert!(
            err.to_string().contains("failed to parse the package file"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_file(&malformed);

        Ok(())
    })
    .await
    .expect("the package commands must answer");
}

/// The trigger catalogue: deploy registers the model's `on` entry, ls and get
/// read it, and start fires it — with and without params.
#[tokio::test(flavor = "multi_thread")]
async fn the_event_commands_read_and_fire_a_manual_trigger() {
    common::with_server("events", true, |_engine, url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        let out = evt::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert!(out.contains("total 0"), "unexpected page: {out}");

        let missing = std::env::temp_dir().join("acts-cli-no-such-model.yml");
        let err = model::deploy(&mut cli, &missing).await.unwrap_err();
        assert!(
            err.to_string().contains("failed to read the model file"),
            "unexpected error: {err}"
        );

        let file = scratch_file("trigger-model.yml", TRIGGER_MODEL);
        model::deploy(&mut cli, &file).await?;
        let _ = std::fs::remove_file(&file);

        // the tree spelling is rendered from the model the server holds
        let out = model::get(&mut cli, "triggered", &Some("tree".to_string())).await?;
        assert!(out.contains("triggered"), "unexpected tree: {out}");

        let out = evt::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert!(out.contains("total 1"), "unexpected page: {out}");
        let out = evt::ls(
            &mut cli,
            &Some(0),
            &Some(5),
            &vec![util::parse_key_value("kind=manual").unwrap()],
            &vec![util::parse_sort("create_time").unwrap()],
        )
        .await?;
        assert!(out.contains("total 1"), "unexpected page: {out}");

        let out = evt::get(&mut cli, "triggered:fired").await?;
        assert!(out.contains("fired"), "unexpected event: {out}");

        // firing starts the model's run and answers the process id
        let out = evt::start(&mut cli, "triggered:fired", &Some(json!({"order": 42}))).await?;
        assert!(out.contains("pid"), "unexpected start answer: {out}");
        // no payload: the trigger's default params are used
        let out = evt::start(&mut cli, "triggered:fired", &None).await?;
        assert!(out.contains("pid"), "unexpected start answer: {out}");

        // the REPL spelling reaches the same command, params parsed as json
        cli.run("event get triggered:fired").await?;
        cli.run(r#"event start triggered:fired --params '{"order": 7}'"#)
            .await?;
        cli.run("proc ls").await?;
        cli.run("task ls").await?;
        cli.run("snapshot ls nothing").await?;

        let err = evt::start(&mut cli, "missing", &None).await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("evt:start"), "unexpected error: {text}");
        let err = evt::get(&mut cli, "missing").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("evt:get"),
            "unexpected error: {err:#}"
        );

        Ok(())
    })
    .await
    .expect("the event commands must answer");
}

/// The message surface over a live subscription: the delivery rows a
/// subscription creates are read, acked by the id the row names, removed, and
/// the subscription ended.
#[tokio::test(flavor = "multi_thread")]
async fn the_message_commands_read_and_settle_deliveries() {
    common::with_server("messages", true, |_engine, url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        let out = msg::ls(&mut cli, &None, &None, &vec![], &vec![]).await?;
        assert!(out.contains("total 0"), "unexpected page: {out}");

        let err = msg::get(&mut cli, "missing").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("msg:get"),
            "unexpected error: {err:#}"
        );

        // the housekeeping actions answer even with nothing to settle
        msg::redo(&mut cli).await?;
        msg::clear(&mut cli, &None).await?;
        msg::clear(&mut cli, &Some("no-such-proc".to_string())).await?;
        let out = msg::rm(&mut cli, "missing").await?;
        assert!(out.contains("(elapsed"), "unexpected remove: {out}");

        // a subscription makes the engine record a delivery row per event
        cli.run("message sub cli-msg").await?;

        let file = scratch_file("message-model.yml", MODEL);
        model::deploy(&mut cli, &file).await?;
        let _ = std::fs::remove_file(&file);
        proc_cmd::start(&mut cli, "simple", &Some("pm1".to_string()), &Vars::new()).await?;

        // an independent connection reads the delivery id the row carries
        let mut probe = client::connect(&url, None).await?;
        let mut delivery = None;
        for _ in 0..600 {
            let resp = probe
                .send::<PageData<MessageInfo>>("msg:ls", Vars::new().with("query", Query::new()))
                .await
                .map_err(|err| client::action_failed("msg:ls", err))?;
            if let Some(row) = resp.data.and_then(|page| page.rows.into_iter().next()) {
                delivery = Some(row.id);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let id = delivery.expect("the subscription must record a delivery row");

        let out = msg::get(&mut cli, &id).await?;
        assert!(out.contains(&id), "unexpected message: {out}");
        let out = msg::ack(&mut cli, &id).await?;
        assert!(out.contains("(elapsed"), "unexpected ack: {out}");

        let out = msg::ls(
            &mut cli,
            &Some(0),
            &Some(10),
            &vec![util::parse_key_value("pid=pm1").unwrap()],
            &vec![util::parse_sort("create_time!").unwrap()],
        )
        .await?;
        assert!(out.contains("total"), "unexpected page: {out}");

        let out = msg::rm(&mut cli, &id).await?;
        assert!(out.contains("(elapsed"), "unexpected remove: {out}");
        let out = msg::unsub(&mut cli, "cli-msg").await?;
        assert!(out.contains("(elapsed"), "unexpected unsub: {out}");

        Ok(())
    })
    .await
    .expect("the message commands must answer");
}

/// Every `act` verb against live irq tasks: the actions the kernel serves move
/// their own run, and what the server refuses — a push without its `uses`, a
/// removal of a decided act, and the legacy rewind guards (a cancel with no
/// forward step, a back from a running task) — is reported as an error naming
/// the action.
#[tokio::test(flavor = "multi_thread")]
async fn the_act_commands_drive_live_tasks() {
    common::with_server("acts", true, |_engine, url| async move {
        let mut channel = client::connect(&url, None).await?;
        let mut cli = CommandRunner::new(&mut channel);

        let file = scratch_file("act-model.yml", ACT_MODEL);
        model::deploy(&mut cli, &file).await?;
        let _ = std::fs::remove_file(&file);

        let pids: Vec<String> = (1..=6).map(|n| format!("pa{n}")).collect();
        let mut tids = Vec::new();
        for pid in &pids {
            proc_cmd::start(&mut cli, "acts", &Some(pid.clone()), &Vars::new()).await?;
            tids.push(wait_task_id(&mut cli, pid).await);
        }

        let out = proc_cmd::get(&mut cli, &pids[0], &Some("tree".to_string())).await?;
        assert!(out.contains(&pids[0]), "unexpected proc: {out}");

        // running -> submitted; a cancel is the legacy rewind, and the
        // server's guard refuses one with no forward package step to rewind —
        // the refusal still names the action the user typed
        cli.run(&format!("act submit {} {} -v answer=42", pids[0], tids[0]))
            .await?;
        let err = cli
            .run(&format!("act cancel {} {}", pids[0], tids[0]))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("act:cancel"),
            "unexpected error: {err:#}"
        );
        // running -> completed, skipped, aborted, failed and backed
        cli.run(&format!("act complete {} {} -v ok=1", pids[1], tids[1]))
            .await?;
        cli.run(&format!("act skip {} {}", pids[2], tids[2]))
            .await?;
        cli.run(&format!("act abort {} {}", pids[3], tids[3]))
            .await?;
        cli.run(&format!(
            "act error {} {} E42 -v detail=oops",
            pids[4], tids[4]
        ))
        .await?;
        // back is the same legacy rewind: a running task is not a backed one
        let err = cli
            .run(&format!("act back {} {} to-step", pids[5], tids[5]))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("act:back"),
            "unexpected error: {err:#}"
        );

        // push names the act it creates: without `uses` the server answers the
        // legacy caller error; a decided act (completed above) takes no removal
        let err = cli
            .run(&format!("act push {} {}", pids[1], tids[1]))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("uses"),
            "unexpected error for act push: {err:#}"
        );
        let err = cli
            .run(&format!("act remove {} {}", pids[1], tids[1]))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("ActionNotAllowed"),
            "unexpected error for act remove: {err:#}"
        );

        // a run the server never saw is a refusal naming the action
        let err = cli.run("act submit missing missing").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("act:submit"),
            "unexpected error: {err:#}"
        );

        Ok(())
    })
    .await
    .expect("the act commands must answer");
}

/// `main`'s own paths, run as the shipped binary: clap's help answer, an
/// argument clap rejects, and a server address that never accepts.
#[test]
fn the_binary_answers_help_and_reports_bad_input_and_unreachable_servers() {
    let dir = scratch_dir("binary-failures");

    let out = run_binary(&["--help"], None, &dir);
    assert!(out.status.success(), "--help must exit successfully");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("acts-cli"), "unexpected help: {stdout}");

    let out = run_binary(&["bogus"], None, &dir);
    assert!(
        !out.status.success(),
        "an unknown argument must be a failure"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bogus"),
        "clap's diagnostic must name the argument: {stderr}"
    );

    // a port nothing listens on: startup reports the address it could not reach
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener
        .local_addr()
        .expect("the bound address")
        .port()
        .to_string();
    drop(listener);
    let out = run_binary(&["--port", port.as_str()], None, &dir);
    assert!(
        !out.status.success(),
        "an unreachable server must be a failure"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed to connect"),
        "the startup error must name the failure: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The REPL loop in the shipped binary against a live server: it authenticates
/// at startup, answers help, runs a command, reports a bad line, and exits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_binary_runs_the_repl_against_a_live_server() {
    common::with_server("binary-repl", false, |_engine, url| async move {
        let port = url
            .rsplit(':')
            .next()
            .expect("the url names a port")
            .to_string();

        let dir = scratch_dir("binary-repl");
        let out = run_binary(&["--port", &port], Some("\nmodel ls\nbogus\nexit\n"), &dir);
        assert!(
            out.status.success(),
            "the REPL must exit successfully: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("authenticated as anonymous"),
            "the startup greeting must name the identity: {stdout}"
        );
        assert!(
            stdout.contains("tap 'help'"),
            "the startup tip must be printed: {stdout}"
        );
        assert!(
            stdout.contains("total 0"),
            "the command's answer must be printed: {stdout}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("bogus"),
            "a rejected line must be reported: {stderr}"
        );

        // an explicit token the server does not accept leaves the caller
        // anonymous, and the startup says so instead of pretending otherwise
        let dir = scratch_dir("binary-anonymous");
        let out = run_binary(
            &["--port", port.as_str(), "--token", "not-a-token"],
            Some("exit\n"),
            &dir,
        );
        assert!(
            out.status.success(),
            "the anonymous REPL must still start: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("connected anonymously"),
            "an unusable token must be reported as anonymous: {stdout}"
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    })
    .await
    .expect("the binary must run the REPL against the server");
}
