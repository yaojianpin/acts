# acts-package-shell

The acts shell package plugin for acts. 

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
inputs:
  my_input:  "hello, world"
steps:
  - name: shell step
    uses: acts.app.shell
    params:
      shell: nu
      content-type: json
      script: |
        let data = "${{ my_input }}"
        $data | split row ',' | each { |it| $it | str trim  } | to json
```

The shell runs asynchronously and does not block the workflow executor, and
one act is bounded on both axes — time and output — by the deployment's
`[shell]` section:

```toml
[shell]
# deadline of one act; when it is reached the shell is killed, reaped, and the
# act fails. 1..=3600000, defaults to 300000 (5 minutes).
timeout-ms = 300000
# bytes captured from each of stdout and stderr; the act fails and the shell is
# killed when a stream exceeds it. 1..=67108864, defaults to 1048576 (1 MiB).
max-output-bytes = 1048576
```

Both bounds are always in force — no value disables them, a value outside its
range fails startup, and an act has no parameter that widens them. They exist
because a script that never exits, or that floods a stream, otherwise holds a
scheduler lane forever: all of the engine's concurrency is a fixed number of
lanes, so one unbounded act is one lane that never comes back.

Neither bound waits for the other to finish. A stream that hits the capture
limit ends the act immediately and the child is killed — a script that keeps
writing is not waited for — and a shell that closed its output but kept running
is killed at the same deadline. In every case the child is killed and waited
for, so the act leaves no zombie behind it.

Cancellation reaches the child too: when the engine shuts down, or an action
(`abort`, `cancel`, `skip`, `remove`, `next`, `error`) overrides the task while
the act runs, the shell is killed and the act reports no failure of its own —
the action owns the task's state, and a shutdown leaves the task for the next
start to resume.

End-to-end tests for all of this live in `tests/bounds.rs`.

## Script policy

`[shell]` in the engine config is an allow/deny pair of globs over the **whole
script text**:

```toml
[shell]
# when non-empty, only a script matching one of these may run
allow = ["ls", "ls *", "cat *.txt", "nu *"]
# always refused, allow or not
deny = ["*rm -rf*", "*sudo *", "*> /etc/*"]
```

`*` matches any run of characters — `/` and newlines included, since a script
is one string and not a path — and `?` matches one. `deny` wins over `allow`;
both lists empty means no restriction. A script the policy refuses fails the act
before anything is spawned, and a pattern that does not compile fails startup
rather than silently governing nothing.

Like the workdir check below, this is **policy, not a sandbox**: a glob over
script text cannot see what the script will do, so it states intent and refuses
the obvious. A hostile workflow needs an OS boundary around the server.

## Directory control

When the engine's ACL config gives the process a workdir (see the
access-control chapter of the book), the script runs inside the directory the
run owns — the configured root plus the process id, `<workdir>/<pid>`: that
directory is its working directory, `HOME`, `TMPDIR`/`TEMP`/`TMP` and `PWD`
point inside it, and `ACTS_WORKDIR` names it. A script that names an absolute
path or a `..` segment is refused before it runs.

That check is policy, not a sandbox — the containment is the child's working
directory, and a shell can spell an outside path in ways no textual check
follows. Treat a hostile workflow as needing an OS boundary around the server.

That directory lives as long as the run's durable rows: the engine removes it
with them once the run finished and its deliveries settled, and a run that
never became durable removes it immediately. A file the run must keep has to be
exported, not left in the workdir.

Without a workdir, no directory control applies: the script inherits the
server's own working directory and environment.
