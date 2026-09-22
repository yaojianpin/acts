# acts-cli

Interactive command-line client for an acts-server. Connects over gRPC
(`acts-channel`) and runs subcommands against the server.

## Run

```bash
cargo run -p acts-cli -- --host 127.0.0.1 --port 10080
```

- `--host` server hostname (default `127.0.0.1`)
- `--port` server gRPC port (default `10080`)
- `--token` ACL token presented on every request (or the `ACTS_TOKEN`
  environment variable, which keeps it out of the process list)

On startup the CLI asks the server who it is and prints the resolved identity,
so a missing or stale token fails immediately instead of on the first command:

```text
$ ACTS_TOKEN="$TOKEN" cargo run -p acts-cli -- --host 127.0.0.1
authenticated as roles: operator
tap 'help' to list available subcommands and some concept guides
```

Type `help` inside the session to list the available subcommands.

## Values and errors

A value a command takes (`--data`, `-p`, `-v`) is JSON when it parses as JSON
— `1`, `null`, `[2, 3]`, `{"a": 1}` — and the string it looks like otherwise,
so `-v name=user1` is the string and needs no quoting; an object or array that
is not valid JSON is reported as the typo it is. Quote a JSON value in the
shell so the session's own tokenizer sees it whole.

A command that fails prints the action and what the server answered, e.g.

```text
model get missing
action 'model:get' failed: code: 'Internal error', message: "cannot find models by 'missing'"
```

and the session continues; only `exit` (or Ctrl-D) leaves it. A line that does
not parse prints clap's own usage diagnostic instead.

## Subcommands

| command    | purpose                                   |
|------------|-------------------------------------------|
| `model`    | deploy / list / get / remove models       |
| `package`  | publish / list / get / remove packages    |
| `proc`     | start / list / get processes              |
| `task`     | list / get tasks                          |
| `message`  | list / get / ack / redeliver messages     |
| `act`      | send act actions (`submit`, `back`, …)    |
| `event`    | get / list / start triggers (events)      |
| `snapshot` | feed and query snapshot data (see below)  |
| `exit`     | leave the session                         |

Every command prints its result and the elapsed time.

### snapshot

```text
snapshot upsert profile --scope u1 --rev 3 --data '{"team":"infra","beta":true}'
snapshot get    profile --scope u1          # yaml of one scope
snapshot ls     profile                     # table of every scope
snapshot remove profile --scope u1          # tombstone
```

- `upsert` writes one value for the target `name` and `scope` (the server
  auto-registers the target with default options the first time).
- `get` answers `snapshot not found` when the scope has no value.
- `remove` deletes the value so later task prepares stop sealing it.

The JSON for `--data` is parsed by the shell tokenizer first, so quote it
(e.g. `'{"a":1}'`).

## Example session

```text
$ cargo run -p acts-cli -- --host 127.0.0.1 --port 10080
tap 'help' to list available subcommands and some concept guides
127.0.0.1:10080 $ snapshot upsert profile --scope acme --rev 1 --data '{"rate_limit":100}'
snapshot 'profile' scope 'acme' updated (rev 1)(elapsed 5ms)
127.0.0.1:10080 $ snapshot get profile --scope acme
data:
  rate_limit: 100
rev: 1
scope: acme
timestamp: 1788858659635
(elapsed 1ms)
127.0.0.1:10080 $ exit
```
