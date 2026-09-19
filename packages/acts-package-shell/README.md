# acts-package-shell

The acts shell package plugin for acts: runs a workflow's script in
[bashkit](https://github.com/everruns/bashkit), a virtual bash whose filesystem
is the run's own directory.

## Installation

```bash
cargo add acts-package-shell
```

## Start
```rust,no_run
use acts::Engine;
use acts_package_shell::ShellPackagePlugin;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .add_plugin(&ShellPackagePlugin)
        .start();
}
```

## Example

```yaml
name: shell example
id: shell-example
vars:
  - name: my_input
    value: "hello, world"
steps:
  - name: shell step
    uses: acts.app.shell
    params:
      shell: bash
      content-type: json
      script: |
        printf '{"data": "%s"}\n' "${{ my_input }}"
```

`shell` is `bash` and nothing else. The interpreter is bashkit's virtual bash —
no process is spawned and no other shell exists here — so a workflow naming
`sh`, `nu` or `powershell` fails when its params are read instead of being run
as bash.

With no workdir the script still runs: it gets bashkit's own in-memory
filesystem, a sandbox with no host behind it.

## Bounds

One act is bounded on both axes — time and output — by the deployment's
`[shell]` section:

```toml
[shell]
# deadline of one act; when it is reached the interpreter is stopped and the
# act fails. 1..=3600000, defaults to 300000 (5 minutes).
timeout-ms = 300000
# bytes captured from each of stdout and stderr; the act fails when a stream
# reaches it. 1..=67108864, defaults to 1048576 (1 MiB).
max-output-bytes = 1048576
```

Both bounds are always in force — no value disables them, a value outside its
range fails startup, and an act has no parameter that widens them. They exist
because a script that never ends, or that floods a stream, otherwise holds a
scheduler lane forever: all of the engine's concurrency is a fixed number of
lanes, so one unbounded act is one lane that never comes back.

The interpreter reports both in the act's own words: a script the deadline
stopped fails with `timed out after <timeout-ms>`, and a stream that reached the
capture limit fails with `max-output-bytes limit (<max-output-bytes>)`. The
package stops the interpreter in both cases, so nothing of the script keeps
running behind the failed act.

Underneath, the interpreter's own limits stay in force (its defaults): a script
that never yields to the runtime — a tight loop the deadline cannot reach — is
stopped by the interpreter's command, loop and work counters instead.

Cancellation reaches the interpreter too: when the engine shuts down, or an
action (`abort`, `cancel`, `skip`, `remove`, `next`, `error`) overrides the task
while the act runs, the script is stopped and the act reports no failure of its
own. Because the interpreter runs in this process, the act gives its work up at
once rather than after killing and reaping a shell: a shutdown that reaches the
run before its store closes can therefore settle the interrupted act as
completed — the engine's decision — where a child process usually lost that race
and left the run for the next engine to resume.

End-to-end tests for the bounds live in `tests/bounds.rs`, for what a script's
filesystem is in `tests/workdir.rs`, and for the engine's failure modes (a
shutdown, a store that refuses a write) in `tests/reliability.rs`.

## Script policy

`[shell]` in the engine config is an allow/deny pair of globs over the **whole
script text**:

```toml
[shell]
# when non-empty, only a script matching one of these may run
allow = ["ls", "ls *", "cat *.txt"]
# always refused, allow or not
deny = ["*rm -rf*", "*sudo *", "*> /etc/*"]
```

`*` matches any run of characters — `/` and newlines included, since a script
is one string and not a path — and `?` matches one. `deny` wins over `allow`;
both lists empty means no restriction. A script the policy refuses fails the act
before anything runs, and a pattern that does not compile fails startup rather
than silently governing nothing.

This is **policy, not a sandbox**: a glob over script text cannot see what the
script will do — `a=rm; $a -rf /` names no forbidden word. It states intent and
refuses the obvious; what a script can actually reach is decided by its
filesystem (below).

## Directory control

When the engine's ACL config gives the process a workdir (see the
access-control chapter of the book), that directory is the **root of the
script's filesystem**: the configured root plus the process id,
`<workdir>/<pid>`, is `/` inside the script. `pwd` is `/`, a relative path
resolves inside it, and `/` is the only directory tree the script can name — the
rest of the host filesystem is not part of the filesystem it was given, so no
textual check is needed to keep it out.

The two names are one file, not a copy: a file the host puts in
`<workdir>/<pid>` is readable by the script at the same relative path, and a
file the script writes there is the file the host sees. `HOME`, `PWD`,
`TMPDIR`/`TEMP`/`TMP` point at the root for the tools that default to them, and
`ACTS_WORKDIR` names it (`/`) for the script.

That directory lives as long as the run's durable rows: the engine removes it
with them once the run finished and its deliveries settled, and a run that never
became durable removes it immediately. A file the run must keep has to be
exported, not left in the workdir.

Without a workdir, no host directory is mounted at all: the script runs on the
interpreter's in-memory filesystem, so its writes succeed against a filesystem
that has no host behind it and leaves nothing behind.
