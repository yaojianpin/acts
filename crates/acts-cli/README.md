# acts-cli

Interactive command-line client for an acts-server. Connects over gRPC
(`acts-channel`) and runs subcommands against the server.

## Run

```bash
cargo run -p acts-cli -- --host 127.0.0.1 --port 10080
```

- `--host` server hostname (default `127.0.0.1`)
- `--port` server gRPC port (default `10080`)

Type `help` inside the session to list the available subcommands.

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
