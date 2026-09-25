# acts-cli

Interactive command-line client for an acts-server. Connects over gRPC
(`acts-channel`) and runs subcommands against the server.

## Run

```bash
cargo run -p acts-cli -- --host 127.0.0.1 --port 10080
```

- `--host` server hostname (default `127.0.0.1`)
- `--port` server gRPC port (default `10080`)
- `--token` an explicit session token presented on every request (or the
  `ACTS_TOKEN` environment variable, which keeps it out of the process list).
  It is used as given — it is the caller's, not the CLI's to second-guess.
- `--user` / `-u` log in as this user before entering the REPL (or `ACTS_USER`),
  with `--password` / `ACTS_PASSWORD`, or a prompt when neither is set.

Failing an explicit token, a **stored session** for exactly this server is
reused (see below), and an expired access token is rotated with its refresh
token on the first request. On startup the CLI asks the server who it is
(`acl:whoami`) and prints the resolved identity, so a missing or stale
credential is visible immediately instead of on the first command:

```text
$ ACTS_USER=alice ACTS_PASSWORD=s3cret cargo run -p acts-cli -- --host 127.0.0.1
authenticated as alice
tap 'help' to list available subcommands and some concept guides
```

An administrator prints `authenticated as admin (unrestricted)`. When no
session was established at all, the greeting says so and names the way in
instead of reading like a name:

```text
connected anonymously to http://127.0.0.1:10080: the catalogue reads are available, nothing else. Log in with 'auth login <user>' or --user.
```

Type `help` inside the session to list the available subcommands.

## Authentication

Users live in the server's store and are managed at runtime — the shipped
registry is `acts-acl`'s `UserAcl`, installed by `acts-server` on its engine
builder — and a server that has one starts with the builtin `admin` (password
from `ACTS_ADMIN_PASSWORD`, or generated and printed to the log once on the
first start of a fresh store). A server whose engine carries no registry
answers every login with "no user registry is installed in this engine".

```bash
# log in and store the session so the next run reuses it
acts-cli auth login alice
ACTS_PASSWORD=s3cret acts-cli auth login alice

# the identity the server resolved for this session
acts-cli auth whoami

# drop the session
acts-cli auth logout
```

The session is kept in `session.json` under `$ACTS_CONFIG_DIR`, else
`$HOME/.acts` (`$USERPROFILE` on Windows), else `./.acts`; it is written with
owner-only permissions on unix, and `auth logout` removes it.

`auth user …` administers the server's user registry: `set`/`rm` are `admin`
actions and `ls`/`get` reads, so the caller needs the matching grant — the
server refuses everyone else, the client does not decide first:

```bash
acts-cli auth user ls
acts-cli auth user get alice
acts-cli auth user set alice \
    --allow 'model:*' --allow @read \
    --deny 'model:rm' \
    --pattern 'orders:*' \
    --snapshot 'secrets=$subject' --snapshot 'profile=$subject/*' \
    --password s3cret
acts-cli auth user set alice --rm-password old-secret --disable
acts-cli auth user set alice --enable
acts-cli auth user rm alice
```

- `--allow` / `--deny` take command patterns (`model:*`, `proc:start`) and
  catalog groups (`@read`, `@deploy`, `@execute`, `@write`, `@all`) and
  **replace** the list; `deny` wins, and `@all` in `deny` refuses everything.
- `--pattern` sets the resource-name (`rn`) patterns a workflow must match to
  be deployed or started by this user (repeat it for several; the list replaces
  the previous one).
- `--snapshot TARGET=GLOB[,GLOB]` grants snapshot scopes per target;
  `$subject` is the user name.
- `--password` adds a password, `--rm-password` removes one by its plaintext,
  `--disable`/`--enable` switch the user (login is refused and live sessions
  die while disabled).
- `auth user rm` deletes a user; the builtin `admin` cannot be deleted or
  disabled.

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
| `auth`     | log in / out, show the identity, manage users |
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
