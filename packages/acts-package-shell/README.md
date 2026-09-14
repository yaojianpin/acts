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

The shell runs asynchronously and does not block the workflow executor.
Output capture is unbounded unless `max-output-bytes` is set. That option
limits the number of bytes captured from each of stdout and stderr and fails
the act when the limit is exceeded.

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
