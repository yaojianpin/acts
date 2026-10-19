//! What an act's output becomes: the default `text` says stdout is the value
//! verbatim, `json` parses it, and a script refused by the deployment's policy
//! or naming another interpreter never runs at all.

mod support;

use acts::{Engine, Principal};
use support::{Outcome, block, deploy_text, engine, run, scratch};

async fn run_one(engine: &Engine, principal: &Principal, mid: &str) -> Outcome {
    let outcomes = run(engine, principal, &[mid]).await;
    assert_eq!(outcomes.len(), 1, "one run reports one outcome");
    outcomes.into_iter().next().unwrap()
}

fn workflow(mid: &str, params: &str) -> String {
    format!(
        "name: shell content\nid: {mid}\nver: \"0.1.0\"\nsteps:\n  - id: s1\n    uses: acts.app.shell\n    params:\n{params}"
    )
}

#[tokio::test]
async fn json_content_parses_stdout() {
    let dir = scratch("content-json");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &workflow(
            "shell-json",
            &format!(
                "      shell: bash\n      content-type: json\n      script: |\n{}",
                block(r#"printf '{"answer":42}'"#)
            ),
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-json").await;
    assert!(
        !outcome.failed,
        "the run must complete: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains(r#""answer":42"#),
        "stdout was not parsed as json: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

#[tokio::test]
async fn json_content_rejects_stdout_that_is_not_json() {
    let dir = scratch("content-bad-json");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &workflow(
            "shell-bad-json",
            &format!(
                "      shell: bash\n      content-type: json\n      script: |\n{}",
                block("printf 'not json'")
            ),
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-bad-json").await;
    assert!(outcome.failed, "a non-json body must fail the act");
    assert!(
        outcome.outputs.contains("failed to convert data to json"),
        "{}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

#[tokio::test]
async fn a_script_refused_by_the_policy_never_runs() {
    let dir = scratch("content-policy");
    let (engine, principal) = engine(&dir, "[shell]\ndeny = [\"*rm -rf*\"]").await;
    deploy_text(
        &engine,
        &principal,
        &workflow(
            "shell-denied",
            &format!(
                "      shell: bash\n      script: |\n{}",
                block("rm -rf /tmp/never")
            ),
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-denied").await;
    assert!(outcome.failed, "the denied script must not run");
    assert!(
        outcome.outputs.contains("refused by the [shell] policy"),
        "{}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A params object the package cannot deserialize is refused where its params
/// are read — the schema admits a shell with no script, so this is the
/// package's own check rather than the engine's schema validation.
#[tokio::test]
async fn params_without_a_script_are_refused() {
    let dir = scratch("content-script");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &workflow("shell-no-script", "      shell: bash\n"),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-no-script").await;
    assert!(outcome.failed, "a scriptless act must not run");
    assert!(
        outcome.outputs.contains("invalid ActPackage"),
        "{}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
