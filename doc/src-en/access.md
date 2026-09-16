# Access Control

Every operation of the engine is checked against the identity of the caller
performing it. An `[acl]` section in the engine config is what names those
callers. **Without it, the engine answers to anyone and hands out only the
catalogue**: every request is attributed to the built-in `anonymous` subject,
which may list and get **models and packages** and nothing else — no other
read, no write, no control action, no admin action, no snapshot scope, no
subscription. An unconfigured deployment is for looking at what is deployed,
not for changing it or for reading what a run, a message or a trigger holds.

Two ways out of that default:

- add `[acl]` — the smallest useful section is one `token`, which grants that
  token everything (the `requirepass` equivalent) and switches every caller
  from anonymous to authenticated;
- write `enabled = false` inside `[acl]`, the explicit opt-out: nothing is
  enforced and every caller is unrestricted. That is the pre-ACL behaviour, and
  it is a deliberate choice rather than the absence of a section. An embedder
  says the same thing with `Engine::builder().disable_acl()`, which is the
  setting a test or a local demo uses.

There is no *implicit* unrestricted policy anywhere: not a missing section, not
a process nobody claimed, not a snapshot scope nobody named. Each of those is
the read-nothing case described where it appears below.

## Tokens and roles

A request carries a token; the token selects a **role**; the role's `allow` /
`deny` **action patterns** decide. `deny` always wins. A request with no token
— or with a token matching no role — is refused unless `default_role` names a
role to fall back to (`[[acl.role]] name = "anonymous"` is how a deployment
keeps the read-only default while configuring everything else around it).

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
| subscribe | `msg:sub` |
| admin | `model:rm` `pack:rm` `msg:rm` `msg:clear` `msg:redo` `msg:unsub` |
| embedded | `ext:register_var` (and `pack:publish` for `ext().register_package`) |

The last row is the embedder's own surface — installing a user var module into
the expression environment, publishing a package definition. No wire action
maps to it, so a transport caller cannot reach it; an embedder that extends the
engine it hosts passes its own principal (see
[The executor](#the-executor)).

`allow = ["*"]` is unrestricted: every action passes, and every snapshot scope
too. `acl:whoami` reports the caller's own identity and effective patterns; it
is implicitly allowed for an authenticated caller, so it works as a startup
check without widening anything.

The `anonymous` subject an engine without `[acl]` resolves to gets exactly
`model:ls` `model:get` `pack:ls` `pack:get` — the catalogue. That is the least a
caller the engine cannot name may be trusted with, and the list is pinned by a
test rather than left to interpretation. Everything else is out, including
reads a named caller would take for granted: a process row names who ran what,
a delivery names who was meant to receive it, a trigger names the model it will
start — a caller the engine cannot identify is not the one those rows are about.
`msg:sub` is out for the same reason and one more (a stream both carries live
payloads and stores a delivery row per message for a channel it holds), and
`snap:get`/`snap:ls` are out because a snapshot scope has an owner only when a
policy names one.

## The executor

The engine's operations are grouped on one object, the executor, and **every
one of its methods is checked before it runs** — `model().deploy()`,
`proc().start()` and the rest. It is bound to a caller when it is created:

```rust
// a transport's request, after its token resolved
let executor = engine.executor(&principal);
executor.proc().start("my_model", vars).await?;

// a request that carried no token
let executor = engine.executor(&engine.anonymous());

// the engine's own work, and what a test or a local demo passes
let executor = engine.executor(&Principal::unrestricted());
```

The executor also decides what a run it starts *carries*: `proc().start()` and
`evt().start()` seal the principal's snapshot scopes and workdir root into the
process, so a caller cannot widen its own reading by putting an authority in the
request. Two callers of the same model therefore read what each of them owns.

An embedder is not outside this: it reaches the engine through an executor like
everyone else, so its operations are checked against the principal it passed.
Passing `Principal::unrestricted()` is a statement ("this is the engine's own
work", or "this deployment opted out") and it is written at the call site.

## Snapshot scope ownership

A snapshot target is addressed by `target` × `scope` ([snapshot-backed sealed
data](./model/act.md)). The `snapshot` table of a role narrows which scopes of
which targets that role owns; `$subject` is replaced by the role name, so
`["$subject"]` means "my own scope only".

The rule is enforced twice:

- On the `snap:*` actions — a read or write of a scope outside the caller's
  set is refused, and `snap:ls` returns only the scopes the subject owns, so
  one tenant cannot enumerate another's.
- At **seal time** — a process carries its starter's rules, and the scheduler
  re-checks them before freezing a snapshot value into a task. A workflow
  therefore cannot read another subject's data by being started with someone
  else's `uid`.

A run's authority comes from one place: the principal whose executor started
it. A subflow inherits its parent's authority, so it can never read more than
the run that opened it. A run the engine started with no caller at all — a
`schedule` trigger, an embedder calling `Runtime::start` directly — carries
*no* authority and reads no snapshot scope: an absent authority is not an
unlimited one, and a task that needs owned data fails at its seal with the
subject it lacks instead of being handed the data plane.

## Message face

Messages and their deliveries are authorized by **action grants**, like every
other operation — a process's owner has no say over who may read or ack the
messages it emitted.

- **Subscribing is an action.** Opening a stream (gRPC `on_message`, SSE
  `/msg/sse`) requires `msg:sub` in the role's `allow` list; without it the
  transport answers `PERMISSION_DENIED` / `403` instead of a stream. The
  transport hands the client id it received to that action and registers the
  channel under the key the action answers with, so the checked path and the
  occupied key cannot drift apart.
- **The key is namespaced by subject**: `{subject}/{transport id}` — the
  subject as the prefix, the transport's own client id (SSE keeps its
  `acts-flow-client-` segment) behind it. A second caller naming a client id
  another subject already uses therefore subscribes to a *different* channel
  instead of replacing that subject's handler. `msg:unsub` composes the same
  key from the same id, so a caller only names channels in its own namespace —
  and a role name carrying `/` is refused at config load, which is what keeps
  the prefix unambiguous.
- **Delivery follows the filters and the grants.** A channel receives every
  message that matches its self-declared `type`/`state`/`uses`/`options`
  globs, whoever started the emitting process; what a caller may do with the
  messages it receives is decided by the actions it holds. A role that must not
  read message payloads simply has no `msg:sub` (and no `msg:ls`/`msg:get`).
- **A subscription's backlog is bounded.** Every subscriber has one fixed-size
  queue (`[grpc].queue_size`, default 128; `[web].queue_size`, default 100): a
  delivery never waits for the client, and a full queue means the client
  stopped reading, so that subscription is disconnected — the engine does not
  leave one waiting task per message that did not fit. What the engine still
  owes the channel is not dropped with it: a delivery that was handed over but
  not acked, of a process that has not settled, is re-sent by the retry timer
  once the client subscribes again under the same id (which composes the same
  channel key). As after any disconnect, a channel only receives messages
  emitted while it is registered.
- `msg:ack` and `msg:unsub` are ordinary actions too: any role granted them may
  ack any delivery id and unsubscribe any channel in its own namespace. The
  delivery id is not addressed per client, so grant `msg:ack` the way you would
  grant a write — its holder can silence another caller's unacked messages.

## What is enforced where

Four kinds of rule, and each is checked at the layer that can express it:

| Rule | Enforced on | By |
| --- | --- | --- |
| action permission | every operation, in one table | role `allow`/`deny` patterns |
| snapshot scope ownership | `snap:*` actions, and again at seal time | role `snapshot` table (`$subject`) |
| channel namespace | channel key and `msg:unsub` | the authenticated subject |
| directory confinement | the process workdir | `[acl]`/role `workdir` |

Every operation goes through the action table, which is what makes the check
universal: a transport resolves a token into a principal, and so does an
embedder — the executor an operation runs on carries that principal, and the
dispatch table and the executor match the same action names, defined once per
operation. The two callers that do not *have* an identity are the two the table
has an answer for anyway:

- a request with no token resolves to `default_role`, or to the read-only
  `anonymous` subject when the engine has no `[acl]` section — it is still a
  caller, and it is checked like any other;
- a run nothing claims (a `schedule` trigger) and a subflow (which inherits its
  parent's authority) are the engine's own starts, with no caller to check:
  what they may read is the authority they ended up carrying, and neither can
  gain one from the outside.


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

The shell package has a second, script-level policy of its own — two glob lists
over the **whole script text**:

```toml
[shell]
# when non-empty, only a script matching one of these may run
allow = ["ls", "ls *", "cat *.txt"]
# always refused, allow or not
deny = ["*rm -rf*", "*sudo *"]
```

`*` matches any run of characters, `/` and newlines included, and `deny` wins.
Both lists empty means no restriction; a pattern that does not compile fails
startup. Like the workdir check it is policy rather than a sandbox — a glob
over script text cannot see what the script will do (`a=rm; $a -rf /` names no
forbidden word), so it is for stating intent and refusing the obvious, and a
hostile workflow still needs an OS boundary.

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

`connect` is the tokenless form: it is the anonymous caller, so against a server
without `[acl]` it can read and nothing else, and against a configured one it is
refused unless a `default_role` admits it.
