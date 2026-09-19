//! End-to-end proof of what a script's `${{ ... }}` placeholders become.
//!
//! The script text is filled before the interpreter sees it: the engine
//! evaluates every `${{ ... }}` in the act's params — `Task::params` fills them
//! from the task's own scope — and hands the package the resulting text, so
//! what the script runs is the script with each placeholder replaced in place.
//! Nothing here is the package's own doing, and that is exactly the contract a
//! workflow author relies on: a task var written in the script reaches it.
//!
//! Each case is a real run through the engine, read back from the run's
//! terminal event.

mod support;

use acts::{Engine, Principal};
use support::{Outcome, block, deploy_text, engine, run, scratch};

/// Run the one deployed act `mid` and answer how it ended.
async fn run_one(engine: &Engine, principal: &Principal, mid: &str) -> Outcome {
    let outcomes = run(engine, principal, &[mid]).await;
    assert_eq!(outcomes.len(), 1, "one run reports one outcome");
    outcomes.into_iter().next().unwrap()
}

/// A workflow var is in the script where the placeholder was, verbatim —
/// spaces and all, since what is substituted is the value's text.
#[tokio::test]
async fn a_workflow_var_embeds_in_the_script() {
    let dir = scratch("vars-workflow");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &format!(
            r#"name: shell vars
id: shell-var
ver: "0.1.0"
vars:
  - name: my_input
    value: "hello, world"
steps:
  - id: s1
    uses: acts.app.shell
    params:
      shell: bash
      script: |
{script}
"#,
            script = block("echo \"value=[${{ my_input }}]\"")
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-var").await;
    assert!(
        !outcome.failed,
        "the run must complete: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains("value=[hello, world]"),
        "the var did not reach the script: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A step var and a previous step's output are both reachable through
/// `$inputs()`: a step's own vars are merged into its inputs, and the previous
/// step's outputs are the inputs of the next one.
#[tokio::test]
async fn the_inputs_of_the_act_embed_in_the_script() {
    let dir = scratch("vars-inputs");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &format!(
            r#"name: shell vars
id: shell-inputs
ver: "0.1.0"
steps:
  - id: s1
    uses: acts.app.shell
    params:
      shell: bash
      script: |
{first}
  - id: s2
    vars:
      - name: step_input
        value: "from the step"
    uses: acts.app.shell
    params:
      shell: bash
      script: |
{second}
"#,
            first = block("printf hello"),
            second = block(
                "echo \"prev=[${{ $inputs().data }}]\"\necho \"step=[${{ $inputs().step_input }}]\""
            )
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-inputs").await;
    assert!(
        !outcome.failed,
        "the run must complete: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains("prev=[hello]"),
        "the previous step's output did not reach the script: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains("step=[from the step]"),
        "the step's own var did not reach the script: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// Several placeholders in one script are each replaced in place, and the rest
/// of the text reaches bash untouched: the shell's own `$x` and `${x}` are
/// still the shell's, so interpolation and the interpreter do not fight over
/// the same text.
#[tokio::test]
async fn every_placeholder_embeds_and_the_interpreter_keeps_its_own() {
    let dir = scratch("vars-many");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &format!(
            r#"name: shell vars
id: shell-many
ver: "0.1.0"
vars:
  - name: my_input
    value: "hello, world"
steps:
  - id: s1
    uses: acts.app.shell
    params:
      shell: bash
      script: |
{script}
"#,
            script = block(
                "shell_var=\"from the script\"\n\
                 echo \"shell=[$shell_var] brace=[${shell_var}]\"\n\
                 echo \"var=[${{ my_input }}]\"\n\
                 echo \"sum=[${{ 1 + 2 }}]\"\n\
                 echo \"join=[${{ \"a\" + \"b\" }}]\""
            )
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-many").await;
    assert!(
        !outcome.failed,
        "the run must complete: {}",
        outcome.outputs
    );
    for expected in [
        "shell=[from the script] brace=[from the script]",
        "var=[hello, world]",
        "sum=[3]",
        "join=[ab]",
    ] {
        assert!(
            outcome.outputs.contains(expected),
            "{expected} is missing — a placeholder was missed or the shell's own expansion \
             was touched: {}",
            outcome.outputs
        );
    }

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}

/// A placeholder whose expression cannot be evaluated does not fail the act:
/// the fill turns it into the JSON `null` and the script runs with that text.
/// A misspelled var name therefore reaches the script as `null` instead of
/// stopping the run, which is the part a workflow author has to know.
#[tokio::test]
async fn an_unresolvable_expression_embeds_as_null() {
    let dir = scratch("vars-null");
    let (engine, principal) = engine(&dir, "").await;
    deploy_text(
        &engine,
        &principal,
        &format!(
            r#"name: shell vars
id: shell-null
ver: "0.1.0"
steps:
  - id: s1
    uses: acts.app.shell
    params:
      shell: bash
      script: |
{script}
"#,
            script = block("echo \"v=[${{ missing_name }}]\"")
        ),
    )
    .await;

    let outcome = run_one(&engine, &principal, "shell-null").await;
    assert!(
        !outcome.failed,
        "an expression that cannot be evaluated must not fail the act: {}",
        outcome.outputs
    );
    assert!(
        outcome.outputs.contains("v=[null]"),
        "the unresolved expression must reach the script as null: {}",
        outcome.outputs
    );

    std::fs::remove_dir_all(&dir).ok();
    engine.close().await;
}
