# Access Control

Every operation of the engine is checked against the identity of the caller
performing it. Access control is **always on**, and it is no longer configured
in the config file: an `[acl]` section left in `acts.toml` is ignored with a
warning, and there is no config token, no role and no default role. **Users live
in the engine's store and are managed at runtime** — created, granted and
removed with the `acl:*` actions (`acts-cli auth user …` on top of them) — and
an engine that installs the user registry starts with the builtin `admin` (a
bare engine has no users at all; see [Where the users live](#where-the-users-live)).

A request the engine cannot attribute to a user — no token at all, or an
unknown/expired one — is the **anonymous** caller, and it may only list and get
**models and packages**: no other read, no write, no control action, no admin
action, no snapshot scope, no subscription. An unconfigured deployment is for
looking at what is deployed, not for changing it or for reading what a run, a
message or a trigger holds. Everything else requires a login.

The one deliberate opt-out is written in code, not config:
`Engine::builder().disable_acl()` makes every caller unrestricted — the
pre-ACL behaviour, the setting a test, a demo or a single-tenant embedder uses.
It is a statement at the call site rather than the absence of a setting, and
logging in is then refused with "the acl is disabled: requests need no login".

There is no *implicit* unrestricted policy anywhere: not an incomplete grant
list, not a process nobody claimed, not a snapshot scope nobody named. Each of
those is the read-nothing case described where it appears below.

## Where the users live

The engine keeps the *policy* and the *registry* apart, in two crates:

- **`acts`** holds what the engine enforces. `acts::Principal` is the compiled
  answer — it decides whether an action, a snapshot scope and a resource name
  are allowed — and `acts::AccessControl` is the port the engine talks through:
  `load`, `enabled`, `authenticate`, `anonymous`, `login`, `refresh`, `logout`,
  `set_user`, `del_user`, `user_names`, `get_user`. `Engine::acl()` hands back
  the installed one as `Arc<dyn AccessControl>`. `acts::UserPolicy` is the
  plain-data form of a user's grants, and `Principal::from_policy` compiles it.
- **`acts-acl`** holds the shipped implementation: `acts_acl::UserAcl`, a
  store-backed user registry and session store. Installing it is one call:

```rust
use acts::Engine;
use acts_acl::AclUsers;

let engine = Engine::builder().with_user_acl().start().await?;
```

A registry built by hand goes in through `EngineBuilder::set_acl`, e.g.
`Engine::builder().set_acl(Arc::new(acts_acl::UserAcl::new()))`; the trait is
public, so an embedder may install its own implementation instead.

A **bare engine** — `Engine::builder().start()` with nothing installed — runs
`acts::AnonymousAcl`: it knows no users, every caller is the anonymous
catalogue-only principal, and `acl:login` is refused with "no user registry is
installed in this engine". That is a deliberate default rather than a hole: an
embedder that never installs a registry cannot be logged into, so nobody can
become anyone. `acts-server` installs `acts_acl::UserAcl` in its engine
builder, so the shipped server is the behaviour this chapter describes.

`UserAcl` keeps two collections in the engine's own store, under prefixes of
their own:

| Prefix | One document per | Holds |
| --- | --- | --- |
| `acl_users` | user | the name (also the subject), the salted password hashes (`salt$sha256(salt:password)`), the allow/deny/patterns/snapshot grants, an on/off flag |
| `acl_sessions` | live login | the sha256 digests of the access and refresh tokens with both expiries (the access digest is the row id) |

They are ordinary store documents, written through the same store the workflow
rows use — the same backend, database lease and writer shards — so a restart
keeps its users and its sessions. Only the sha256 digests of tokens are stored;
the clear text exists in transit and in the client's own session file.

The names and lifetimes the rest of this chapter uses are this crate's
constants: `acts_acl::ADMIN_USER` (`admin`), `acts_acl::ADMIN_PASSWORD_ENV`
(`ACTS_ADMIN_PASSWORD`), `acts_acl::ACCESS_TOKEN_TTL_SECS` (3600) and
`acts_acl::REFRESH_TOKEN_TTL_SECS` (604800).

## Users, passwords and the builtin admin

A user is a row in the store (`acl:setuser` creates or updates it,
`acl:deluser` removes it, `acl:getuser`/`acl:users` read it back). `acl:setuser`
takes an `acts::UserSpec` — the same fields as below, where `None` means "leave
this setting alone" and a list replaces the current one — and writes an
`acts_acl::AclUser` row. `acl:getuser` answers the policy without the password
hashes: name, `enabled`, `allow`, `deny`, `patterns`, `snapshot`, the number of
passwords, and whether the user is unrestricted. A user carries:

| Field | Meaning |
| --- | --- |
| `passwords` | one or more passwords; each is stored salted (sha256 with a per-password salt), never in clear text. Several entries let you rotate without downtime: add the new one, then remove the old. |
| `allow` / `deny` | the command and catalog patterns that decide which **actions** the user may run (`deny` wins). |
| `patterns` | resource-name (`rn`) patterns, over the workflows the user may deploy and start. |
| `snapshot` | per target, the scope patterns the user owns. |
| `enabled` | a disabled user cannot log in and its live sessions die. |

The engine always starts with the builtin administrator `admin`:

- it is unrestricted (every action, every snapshot scope, every resource);
- its password is taken from `ACTS_ADMIN_PASSWORD` on the first start of a
  fresh store, or generated and **printed to the log once** when that variable
  is not set — change it with `acl:setuser`;
- it cannot be deleted or disabled, so an engine can never lock itself out of
  its own user registry. `acl:deluser admin` and a spec with `enabled = false`
  are refused, and so is `acts-cli auth user set admin --disable`.

Only a user granted the right can change the registry: `acl:setuser` and
`acl:deluser` belong to the `@write` group, while `acl:getuser`/`acl:users` are
reads (`@read`) — all of them are ordinary actions, checked like any other. Reading
and writing a password is not a thing a transport can do — a password only ever
travels as the payload of `acl:login`.

A user name is also the **subject** the engine attributes work to, and it
prefixes the channel key of every subscription the user opens, so a name
containing a path separator (`/`) is refused when the user is written.

A request is resolved against the user's **current** row, not a copy taken at
login, so a grant added or removed applies to a live session on its next
request; a disabled user stops authenticating at once (its token resolves to
`anonymous`), and `acl:deluser` revokes the deleted user's sessions.

## Login and sessions

A caller becomes someone by logging in:

```jsonc
// acl:login {user, password}
{"token": "…", "refresh_token": "…", "expires_in": 3600, "refresh_expires_in": 604800}
```

- The **access token** (`expires_in`, one hour) rides every request as the
  `authorization: Bearer <token>` credential.
- The **refresh token** (`refresh_expires_in`, seven days) is presented to
  `acl:refresh` `{refresh_token}` when the access token expired; the answer is
  a fresh pair, and **the old one dies with it** — a refresh token is
  single-use, so a leaked one cannot be replayed.
- `acl:logout` `{token}` revokes the session that token belongs to (the access
  or the refresh form) and answers `true`/`false`.
- `acl:login`, `acl:refresh` and `acl:logout` need **no grant**: the
  credentials are the payload, and logging out only ever revokes the caller's
  own credential.
- Tokens are opaque random strings; the store keeps only their sha256 digests,
  so the clear text exists in transit and in the client's own session file,
  nowhere on the server.
- `acl:whoami` needs no grant either: it answers the identity the server
  resolved for the request — `{user, subject, authenticated, unrestricted,
  allow, deny, patterns, scopes, workdir_root}`. An anonymous caller is answered
  too, with `"authenticated": false`, which is what makes it usable as a
  startup check. `workdir_root` is the principal's own filesystem root; it is
  reserved and stays `null`, because the directory root is the engine's global
  `workdir` setting (see [Process directory](#process-directory)).

## Grants

A user's `allow` and `deny` lists hold two kinds of token:

- **command patterns** — globs (`*`, `?`) over the action names of the shared
  dispatch table, so `model:*` covers every model operation and `act:*` every
  act operation. They are the same names the CLI and the channel client use.
- **catalog references** — `@read`, `@deploy`, `@execute` and `@write` name the
  group an action belongs to, and `@all` (`@*`, or `*`) names every group. The
  Redis ACL shape of `+command` and `+@category`.

`deny` wins over `allow`, and the default is **deny**: an action nobody granted
is refused, and an action no group claims is `write` — an unknown operation is
never a read. A catalog token that names no group (the groups are `read`,
`write`, `deploy`, `execute` and `all`, and `acts::CATALOG_GROUPS` is the list)
is refused when the user is written, so a mistyped grant fails loudly instead of
silently granting — or denying — nothing.

### Action groups

| Group | Actions |
| --- | --- |
| `@read` | `model:ls` `model:get` `pack:ls` `pack:get` `proc:ls` `proc:get` `task:ls` `task:get` `msg:ls` `msg:get` `evt:ls` `evt:get` `snap:get` `snap:ls` `msg:sub` `acl:whoami` `acl:getuser` `acl:users` |
| `@deploy` | `model:deploy` `pack:publish` |
| `@execute` | `proc:start` `proc:start_from_model` `evt:start` `act:push` `act:remove` `act:submit` `act:complete` `act:abort` `act:cancel` `act:back` `act:skip` `act:error` `msg:ack` |
| `@write` | `model:rm` `pack:rm` `msg:rm` `msg:redo` `msg:clear` `msg:unsub` `snap:upsert` `snap:remove` `acl:setuser` `acl:deluser` `ext:register_var`, plus `acl:login` `acl:refresh` `acl:logout` (which need no grant at all) |
| `@all` | every action (`@*` and `*` are the same grant) |

The groups separate the four ways a deployment is usually divided: `@read`
looks, `@deploy` puts work into the catalogue, `@execute` runs it and drives it
through its acts, and `@write` changes or destroys stored state — the removals,
the snapshot feeds and the **user registry itself** (`acl:setuser`/`acl:deluser`),
so granting `@write` is an administrative act. An action that no group names is
`write`, because an unknown operation is never a read.

Granting one group never implies another: `@execute` starts and drives runs but
deploys and removes nothing, and `@write` does not let a user run anything.
`@all` in `allow` makes the user unrestricted — every action passes, every
snapshot scope is readable, every resource name may be deployed; in `deny` it
refuses everything, overriding any grant a command pattern would have given.

`ext:register_var` is the embedder's own surface — installing a user var module
into the expression environment. No wire action maps to it, so a transport
caller cannot reach it at all; an embedder that extends the engine it hosts
passes its own principal (see [The executor](#the-executor)). Publishing a
package definition (`ext().register_package`) is the `pack:publish` action, so
it follows `@deploy`.

The `anonymous` subject gets exactly `model:ls` `model:get` `pack:ls`
`pack:get` — the catalogue. That is the least a caller the engine cannot name
may be trusted with, and the list is pinned by a test rather than left to
interpretation. Everything else is out, including reads a named caller would
take for granted: a process row names who ran what, a delivery names who was
meant to receive it, a trigger names the model it will start — a caller the
engine cannot identify is not the one those rows are about. `msg:sub` is out for
the same reason and one more (a stream both carries live payloads and stores a
delivery row per message for a channel it holds), and `snap:get`/`snap:ls` are
out because a snapshot scope has an owner only when a user names one.

### Resource patterns

Which workflows a user may **deploy** and **start** is decided separately from
which actions it may run: a workflow declares the resource it operates on as
`rn`, a colon-separated literal (`orders:eu`, no spaces, no glob characters, no
empty segment), and each user has a list of `patterns` its `rn` must match.

```yaml
name: order flow
id: order-eu
ver: 0.1.0
rn: orders:eu
steps:
  - name: pick
    uses: acts.core.set
    params:
      message: "hello"
```

A user granted `--pattern 'orders:*'` may deploy and start that model; one
granted `--pattern 'orders:us'` may not, and the refusal names the resource
(`resource 'orders:eu' is not allowed for user 'alice'`). A model with an
**empty** `rn` claims no resource, so **only an unrestricted user** may deploy
or start it — claiming nothing is not claiming everything. The check runs at
deploy, at process start and again at trigger start, so both the wire actions
and an embedder that drives the model directly are held to it.

### Snapshot scopes

The `snapshot` field is a table of target → scope patterns, per target; a
`$subject` in a pattern is replaced by the user name, so
`["$subject"]` means "my own scope only" and `["$subject/*"]` means "the
scopes under my own name".

```jsonc
// acl:setuser {user: {name: "alice", allow: ["@read", "snap:upsert"],
//                     snapshot: {"secrets": ["$subject"], "profile": ["$subject/*"]}}}
```

An absent target in the table is not owned at all; an unrestricted user owns
everything.

## The executor

The engine's operations are grouped on one object, the executor, and **every
one of its methods is checked before it runs** — `model().deploy()`,
`proc().start()` and the rest. It is bound to a caller when it is created:

```rust
// a transport's request, after its token resolved
let executor = engine.executor(&principal);
executor.proc().start("my_model", vars).await?;

// a caller that presented no usable token
let executor = engine.executor(&engine.anonymous());

// the engine's own work, and what a test or a local demo passes
let executor = engine.executor(&Principal::unrestricted());
```

The executor also decides what a run it starts *carries*: `proc().start()` and
`evt().start()` seal the principal's snapshot scopes and resource patterns into
the process, so a caller cannot widen its own reading by putting an authority in
the request. Two callers of the same model therefore read what each of them
owns, and a run that starts on an `rn` the user does not own is refused before
it exists.

An embedder is not outside this: it reaches the engine through an executor like
everyone else, so its operations are checked against the principal it passed.
Passing `Principal::unrestricted()` is a statement ("this is the engine's own
work", or "this deployment opted out") and it is written at the call site.

## Snapshot scope ownership

A snapshot target is addressed by `target` × `scope` ([snapshot-backed sealed
data](./model/act.md)). The `snapshot` table of a user narrows which scopes of
which targets that user owns; `$subject` is replaced by the user name, so
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
  `/msg/sse`) requires `msg:sub` in the user's `allow` list; without it the
  transport answers `PERMISSION_DENIED` / `403` instead of a stream. The
  transport hands the client id it received to that action and registers the
  channel under the key the action answers with, so the checked path and the
  occupied key cannot drift apart.
- **The key is namespaced by subject**: `{subject}/{transport id}` — the user
  name as the prefix, the transport's own client id (SSE keeps its
  `acts-flow-client-` segment) behind it. A second caller naming a client id
  another subject already uses therefore subscribes to a *different* channel
  instead of replacing that subject's handler. `msg:unsub` composes the same
  key from the same id, so a caller only names channels in its own namespace —
  and a user name carrying `/` is refused when the user is written, which is
  what keeps the prefix unambiguous.
- **Delivery follows the filters and the grants.** A channel receives every
  message that matches its self-declared `type`/`state`/`uses`/`options`
  globs, whoever started the emitting process; what a caller may do with the
  messages it receives is decided by the actions it holds. A user that must not
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
- `msg:ack` and `msg:unsub` are ordinary actions too: any user granted them may
  ack any delivery id and unsubscribe any channel in its own namespace. The
  delivery id is not addressed per client, so `msg:ack` (`@execute`) is worth
  granting as deliberately as a write — its holder can silence another caller's
  unacked messages. `msg:unsub` is `@write`.

## What is enforced where

Four kinds of rule, and each is checked at the layer that can express it:

| Rule | Enforced on | By |
| --- | --- | --- |
| action permission | every operation, in one table | the user's `allow`/`deny` patterns (command globs and `@`catalog groups) |
| snapshot scope ownership | `snap:*` actions, and again at seal time | the user's `snapshot` table (`$subject`) |
| resource ownership | `model:deploy`, `proc:start`, `proc:start_from_model`, `evt:start` | the user's `patterns` over the workflow's `rn` |
| channel namespace | channel key and `msg:unsub` | the authenticated subject (the user name) |

Every operation goes through the action table, which is what makes the check
universal: a transport resolves a token into a principal, and so does an
embedder — the executor an operation runs on carries that principal, and the
dispatch table and the executor match the same action names, defined once per
operation. The two callers that do not *have* an identity are the two the table
has an answer for anyway:

- a request with no usable token is anonymous — it is still a caller, and it is
  checked like any other: an action outside the catalogue is refused as
  `unauthenticated` (log in), not as `permission denied`;
- a run nothing claims (a `schedule` trigger) and a subflow (which inherits its
  parent's authority) are the engine's own starts, with no caller to check:
  what they may read is the authority they ended up carrying, and neither can
  gain one from the outside.

## Process directory

The process directory root is a **global engine setting**, not part of a user:
`workdir = "/srv/acts"` (top level in `acts.toml`, `Config::workdir()`). It is a
**root**: every process gets its own directory `<workdir>/<pid>`, and the run's
filesystem access is confined to that one. It is created at start, and the
process id becomes a path segment — so a pid that is not one safe component
(empty, `.`, `..`, or containing a path separator or colon) is refused rather
than placing the run outside the root it was given. An empty `workdir` is a
config error rather than "no directory control".

The root travels the same private route as the scope authority — it is part of
it: sealed into the process env under a key the workflow's `$env` proxy
refuses, persisted with the process, and never a start option, so a caller
cannot name the directory a run is confined to. The **run's own directory**
(`<root>/<pid>`) is what an act reads through `Context::workdir()` and what a
script reads as `$env.WORK_DIR` — the same directory under two names, neither
of them the configured root. `$env.WORK_DIR` is engine-owned, so a write to that
name is dropped and a run cannot redefine where it runs. The directory lives
exactly as long as the process's durable rows: the sweeper removes it with them
(a finished run whose delivery errored and awaits a manual retry keeps its row,
and its directory with it), and a start that never became durable removes its
own directory immediately — so nothing left in there outlives the run.

`acts.app.shell` mounts it: the script runs in bashkit's virtual bash with that
directory as the **root of its filesystem**, so `pwd` is `/`, a relative path
resolves inside the run's directory, and `/` is the only tree the script can
name — the rest of the host is not part of the filesystem it was given. The two
names are one file: what the host puts in the run's directory the script reads
at the same relative path, and what the script writes there the host sees.
`HOME`, `TMPDIR`/`TEMP`/`TMP` and `PWD` point at the root, and `ACTS_WORKDIR`
names it (`/`) for the script.

The shell package also has a script-level policy of its own — two glob lists
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
startup. This one is policy rather than a sandbox — a glob over script text
cannot see what the script will do (`a=rm; $a -rf /` names no forbidden word) —
so it is for stating intent and refusing the obvious. What a script can actually
reach is decided by the filesystem it was given, which is the run's own
directory and nothing else of the host. `shell: bash` is the only accepted
interpreter: the package runs bashkit, not PowerShell, Nushell or POSIX `sh`,
and a workflow naming one of those fails when its params are read.

Without `workdir`, no directory is mounted and a shell act runs on the
interpreter's in-memory filesystem instead: its writes succeed against a
filesystem with no host behind it, so the run leaves nothing behind — but it
also keeps nothing, and the script cannot read anything the host holds.

## Transport credentials

| Transport | How the token travels |
| --- | --- |
| gRPC | `authorization: Bearer <token>` metadata on every request, including the `on_message` subscription |
| HTTP | `authorization: Bearer <token>` header; `/health` stays open for probes |
| NATS | the `token` field of the action JSON body — the broker authenticates a connection, not a request |

A refusal is `unauthenticated` (no usable session token) or `permission
denied` (a session, but not the right to the operation): gRPC answers
`UNAUTHENTICATED` / `PERMISSION_DENIED`, the HTTP transport `401` / `403`.

## Clients

The CLI logs in, keeps the session, and administers the user registry:

```bash
# log in and store the session (password from ACTS_PASSWORD, else prompted)
acts-cli auth login alice
ACTS_PASSWORD=s3cret acts-cli auth login alice

# who the server resolved for this session, then drop it
acts-cli auth whoami
acts-cli auth logout

# the user registry (set/rm need the admin grant: the server refuses everyone else)
acts-cli auth user ls
acts-cli auth user get alice
acts-cli auth user set alice \
    --allow @read --allow @deploy --allow @execute \
    --deny 'model:rm' \
    --pattern 'orders:*' \
    --snapshot 'secrets=$subject' --snapshot 'profile=$subject/*' \
    --password s3cret
acts-cli auth user set alice --rm-password old-secret --disable
acts-cli auth user set alice --enable
acts-cli auth user rm alice
```

- `--allow`/`--deny` take the command patterns and `@`catalog groups above and
  **replace** the list; `--pattern` sets the resource-name patterns (repeat it
  for several; the list replaces the previous one); `--snapshot`
  takes `TARGET=GLOB[,GLOB]`; `--password` adds a password and
  `--rm-password` removes one by its plaintext; `--disable`/`--enable` switch
  the user (login is refused and live sessions die while disabled).
- Every `auth user …` command runs as the session's own identity: `set` and
  `rm` need the `admin` grant and `ls`/`get` a read grant. A caller without one
  is refused by the server, never by the client.
- A password typed on the command line is visible to the process list;
  `ACTS_PASSWORD` (and the prompt, when it is omitted) keeps it out.

At startup the CLI reuses a **stored session** for the server, so a login does
not have to be repeated:

```bash
# an explicit token is used as given — it is the caller's, not ours to second-guess
acts-cli --token "$TOKEN"
ACTS_TOKEN="$TOKEN" acts-cli

# otherwise: log in as this user before entering the REPL
acts-cli --user alice --password s3cret
ACTS_USER=alice ACTS_PASSWORD=s3cret acts-cli
```

The session is kept in `session.json` under `$ACTS_CONFIG_DIR`, else
`$HOME/.acts` (`$USERPROFILE` on Windows), else `./.acts`; it is written with
owner-only permissions on unix, and `auth logout` removes it. A stored access
token that expired is rotated with its refresh token on the first request; a
session that no longer authenticates is repaired by an `acl:login` when a user
and password were given, and is otherwise dropped. Before entering the REPL the
CLI resolves an identity through `acl:whoami` and prints it, so a missing or
stale credential is visible at startup instead of on the first command:
`authenticated as alice (unrestricted)` for an administrator,
`authenticated as alice` for an ordinary user, and, when no session was
established,

```text
connected anonymously to http://127.0.0.1:10080: the catalogue reads are available, nothing else. Log in with 'auth login <user>' or --user.
```

```rust,no_run
use acts_channel::ActsChannel;

// connect and present the token on every request, refreshing it when it expires
let mut client = ActsChannel::connect_with_session("http://127.0.0.1:10080", session).await?;

// or: log in with a user and password, and keep nothing
let mut client = ActsChannel::connect_with_password("http://127.0.0.1:10080", "alice", "s3cret").await?;
```

`connect` is the tokenless form: it is the anonymous caller, so it can read the
catalogue and nothing else. An unauthenticated caller is refused as
`unauthenticated` rather than as "not allowed", which is what tells a client to
log in instead of asking for more grants.
