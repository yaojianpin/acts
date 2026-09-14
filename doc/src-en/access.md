# Access Control

An optional `[acl]` section in the engine config turns on access control for
the transport plugins. Without the section nothing is enforced — every request
is allowed, which is the pre-ACL behaviour.

The section's presence is the switch. It can be turned off explicitly with
`enabled = false`, but there is no reason to write the section for that.

## Tokens and roles

A request carries a token; the token selects a **role**; the role's `allow` /
`deny` **action patterns** decide. `deny` always wins, and because the section
is the opt-in, a request with no token — or with a token matching no role — is
refused unless `default_role` names a role to fall back to.

Tokens are compared by **SHA-256 digest**. Write `sha256:<64 hex digits>` to
keep the clear text out of the config file, or write the token itself and let
the server hash it at load:

```toml
[acl]
# role applied to an absent/unknown token; omit to refuse such requests
default_role = "guest"

# shorthand: one unrestricted token (the requirepass equivalent)
token = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"

[[acl.role]]
name = "operator"
tokens = ["sha256:2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"]
allow = ["model:ls", "model:get", "proc:ls", "proc:get", "task:*", "msg:ls",
         "msg:ack", "snap:get", "snap:ls", "acl:whoami"]
deny = ["model:rm", "pack:publish"]
snapshot = { secrets = ["$subject"], profile = ["$subject/*"] }
workdir = "/srv/acts"

[[acl.role]]
name = "guest"
tokens = ["sha256:..."]
allow = ["model:ls", "acl:whoami"]
```

A malformed policy is a startup error, never a silent allow or a silent
refuse: a pattern that does not compile, a token assigned to two roles, a
`default_role` naming no role, or an enabled section where no role declares
any token.

## Action names

`allow`/`deny` match the action names of the shared dispatch table, with `*`
and `?` globs — so `act:*` covers every act operation. They are the same names
the CLI and the channel client use.

| Group | Actions |
| --- | --- |
| read | `model:ls` `model:get` `proc:ls` `proc:get` `task:ls` `task:get` `msg:ls` `msg:get` `evt:ls` `evt:get` `pack:ls` `pack:get` `snap:get` `snap:ls` |
| write | `model:deploy` `pack:publish` `snap:upsert` `snap:remove` |
| control | `proc:start` `proc:start_from_model` `act:push` `act:remove` `act:submit` `act:complete` `act:abort` `act:cancel` `act:back` `act:skip` `act:error` `evt:start` `msg:ack` |
| admin | `model:rm` `pack:rm` `msg:rm` `msg:clear` `msg:redo` `msg:unsub` |

`allow = ["*"]` is unrestricted: every action passes, and every snapshot scope
too. `acl:whoami` reports the caller's own identity and effective patterns; it
is implicitly allowed for an authenticated caller, so it works as a startup
check without widening anything.

## Snapshot scope ownership

A snapshot target is addressed by `target` × `scope` ([snapshot-backed sealed
data](./model/act.md)). The `snapshot` table of a role narrows which scopes of
which targets that role owns; `$subject` is replaced by the role name, so
`["$subject"]` means "my own scope only".

The rule is enforced twice:

- On the `snap:*` actions — a read or write of a scope outside the caller's
  set is refused, and `snap:ls` returns only the scopes the subject owns, so
  one tenant cannot enumerate another's.
- At **seal time** — a process started through an action carries its caller's
  rules, and the scheduler re-checks them before freezing a snapshot value into
  a task. A workflow therefore cannot read another subject's data by being
  started with someone else's `uid`.

A process started in-process (an embedder calling the executor directly) or by
a trigger carries no caller authority and stays unrestricted.

## Directory control

The configured `workdir` — on `[acl]`, or per role to override it — is a **root**:
every process the policy starts gets its own directory `<workdir>/<pid>`, and the
run's filesystem access is confined to that one. It is created at start, and the
process id becomes a path segment — so a pid that is not one safe component
(empty, `.`, `..`, or containing a path separator or colon) is refused rather
than placing the run outside the root it was given.

The root travels the same private route as the scope authority — it is part of
it: sealed into the process env under a key the workflow's `$env` proxy
refuses, persisted with the process, and never a start option, so a caller
cannot name the directory a run is confined to. The **run's own directory**
(`<root>/<pid>`) is what an act reads through `Context::workdir()` and what a
script reads as `$env.WORK_DIR` — the same directory under two names, neither of
them the configured root. `$env.WORK_DIR` is engine-owned, so a write to that
name is dropped and a run cannot redefine where it runs. The directory lives
exactly as long as the process's durable rows: the sweeper removes it with them
(a finished run whose delivery errored and awaits a manual retry keeps its row,
and its directory with it), and a start that never became durable removes its
own directory immediately — so nothing left in there outlives the run.

`acts.app.shell` uses it: the script runs with that directory as its working
directory, `HOME`, `TMPDIR`/`TEMP`/`TMP` and `PWD` point inside it, and
`ACTS_WORKDIR` names it for the script. A script that names an absolute path
(`/etc/passwd`, `C:\Windows`) or a `..` segment is refused before it runs.

That textual check is **policy, not a sandbox**: it is what makes the direct
escape a loud failure instead of a silent success, but a shell can spell a
path in ways no textual check follows (`a=/etc; cat $a/passwd`, a symlink
inside the workdir) — the containment that actually holds is the child's
working directory. Treat a hostile workflow as needing an OS boundary (a
container or namespace around the server); per-process directories keep such
runs from colliding meanwhile.

Without `workdir`, no directory control applies and a process may touch
whatever the server's own account can — the behaviour before this option
existed.

## Transport credentials

| Transport | How the token travels |
| --- | --- |
| gRPC | `authorization: Bearer <token>` metadata on every request, including the `on_message` subscription |
| HTTP | `authorization: Bearer <token>` header; `/health` stays open for probes |
| NATS | the `token` field of the action JSON body — the broker authenticates a connection, not a request |

## Clients

```bash
# CLI: the flag wins over the environment variable
acts-cli --token "$TOKEN"
ACTS_TOKEN="$TOKEN" acts-cli
```

The CLI resolves its identity with `acl:whoami` before entering the REPL, so a
missing or stale token fails at startup instead of on the first command.

```rust,no_run
use acts_channel::ActsChannel;

let mut client = ActsChannel::connect_with_token("http://127.0.0.1:10080", Some(token)).await?;
```

`connect` is the tokenless form, valid only against a server without `[acl]`.
