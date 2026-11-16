# acts-acl

Store-backed users, passwords and login sessions for the
[acts](https://crates.io/crates/acts) workflow engine.

The engine enforces one thing: `acts::Principal` answers whether an action,
a snapshot scope and a resource name are allowed. Who becomes which principal
is this crate's business — a user registry and a session registry kept in the
engine's own store, verified passwords, and session tokens with an expiry and
a single-use refresh. `UserAcl` is the engine's
[`acts::AccessControl`](https://docs.rs/acts/latest/acts/trait.AccessControl.html)
implementation: the engine asks it to resolve a token, to log a caller in or
out, and to read and write the users behind them.

## Install

One call on the builder, through the `AclUsers` trait this crate adds:

```rust
use acts::Engine;
use acts_acl::AclUsers;

# async fn run() -> acts::Result<()> {
let engine = Engine::builder().with_user_acl().start().await?;
# Ok(())
# }
```

A registry built by hand goes in through `acts::EngineBuilder::set_acl`, and an
embedder may install any other `AccessControl` instead:

```rust
use acts::{AccessControl, Engine};
use acts_acl::UserAcl;
use std::sync::Arc;

# async fn run() -> acts::Result<()> {
let acl: Arc<dyn AccessControl> = Arc::new(UserAcl::new());
let engine = Engine::builder().set_acl(acl).start().await?;
# Ok(())
# }
```

An engine without this crate installed runs `acts::AnonymousAcl`: no users, no
login (it answers "no user registry is installed in this engine"), the
anonymous catalogue-only policy for every caller.

## What it stores

Three collections in the engine's store, under prefixes of their own:

| Prefix | Row | Holds |
| --- | --- | --- |
| `acl_users` | `AclUser` | a name, its immutable `subject_id`, the `security_version`/`document_revision`, Argon2id PHC password strings, the command/catalog/resource/snapshot grants, an on/off flag |
| `acl_sessions` | `AclSession` | one session: sha256 digests of its access and refresh tokens, the `session_id`/`family_id` rotation lineage, the subject it was issued for, and its `Active`/`Consumed`/`Revoked` state |
| `acl_tombstones` | `AclTombstone` | the record a deleted user generation leaves behind (subject, name, last security version) |

The prefixes are the rows' `acts::DbCollectionIden::iden()` values, which is why
a crate outside `acts` can own a collection at all:

```rust
use acts::DbCollectionIden;

assert_eq!(acts_acl::AclUser::iden(), "acl_users");
assert_eq!(acts_acl::AclSession::iden(), "acl_sessions");
```

The rows are ordinary store documents, written through the same store the
workflow rows use (a `KvStore` backend, the database lease, the writer
shards), so a server restart keeps its users and its live sessions. A session
row's id is the sha256 of its access token, and the refresh digest is an
indexed field, so a refresh resolves from any instance sharing the store. A
refresh rotation is one guarded store batch: the old row becomes `Consumed`
and the successor is born `Active`, or neither happens. Consumed, revoked and
expired rows stay until the GC pass — they are the replay proof and the audit
trail.

## The builtin admin

The first start of a fresh store creates `admin`
([`ADMIN_USER`](crate::ADMIN_USER)): unrestricted, and never deletable or
disableable, so an engine can never lock itself out of its own registry.

Its first password comes from [`ADMIN_PASSWORD_ENV`](crate::ADMIN_PASSWORD_ENV)
(`ACTS_ADMIN_PASSWORD`) when set — the remote/headless path; an empty or blank
value counts as unset. Without it the admin is created with the documented
default password [`DEFAULT_ADMIN_PASSWORD`](crate::DEFAULT_ADMIN_PASSWORD)
(`"admin"`) and the **initial-state flag**: the account can log in, but the
engine refuses every action except changing that password — `acl:setuser`
naming itself with a new password (logout and whoami stay reachable). The
change drops the old credentials (the default password is gone), clears the
flag and bumps the security version, so the next request must log in again
with the new password. The default value is public documentation, never a
logged secret; set the environment variable to skip the initial state
entirely.

Change the admin's password — or anything else about it — with `acl:setuser`.

## Users and grants

An operator writes users with the `acl:setuser` action, whose payload is an
`acts::UserSpec`: `enabled`, passwords to add and to remove by plaintext,
`force_logout`, and the `allow`/`deny` command or catalog patterns, the `rn`
patterns and the per-target snapshot scopes. A user's grants are stored on the
row and compiled into a principal through `acts::UserPolicy` and
`acts::Principal::from_policy`, so a malformed policy is refused when the user
is written rather than silently ignored at request time.

Passwords are Argon2id PHC strings — one entry per password, up to
`ACTS_ACL_MAX_PASSWORDS` (default 3) — so a password can be rotated without
downtime, and a disabled user authenticates nothing, `acl:login` included.
Every write the registry performs is a compare-and-swap against the row it
read: two concurrent patches never merge, the loser gets a conflict and
re-reads. Disabling a user, changing its passwords or forcing a logout bump
its `security_version`; every session snapshots that version at issue time and
dies on mismatch — the O(1) "log this user out everywhere", with no session
traversal. Deleting a user writes a tombstone; the name can come back, but the
re-created account is a new security subject and no old session or refresh
token ever binds to it.

`acl:getuser` answers the policy without the hashes: name, `enabled`, `allow`,
`deny`, `patterns`, `snapshot`, the number of passwords and whether the user is
unrestricted.

## Login budget

`acl:login` is a public endpoint, so it carries a budget: a global KDF
semaphore (`ACTS_ACL_KDF_CONCURRENCY`, default 8) bounds how many Argon2id
verifications run at once; an unknown user burns the same dummy verification
as a known one, so timing does not enumerate accounts; passwords above 1 KiB
are refused before any KDF work; five failed logins for one name start an
exponential backoff (1 s doubling to a 15-minute ceiling,
`ACTS_ACL_LOGIN_MAX_FAILURES` / `ACTS_ACL_LOGIN_BACKOFF_MAX_SECS`), and a
per-name attempt bucket (5 burst, 10/minute) bounds session-stuffing. Every
refusal answers exactly the same words as a wrong password.

The default Argon2id parameters are the OWASP starting point (19 MiB, 2
passes, parallelism 1); a budgeted deployment raises them with
`ACTS_ARGON2_M_KIB` / `ACTS_ARGON2_T` / `ACTS_ARGON2_P` — the parameters
travel inside every PHC string, so existing hashes keep verifying.

## Sessions

`acl:login` answers an access token
([`ACCESS_TOKEN_TTL_SECS`](crate::ACCESS_TOKEN_TTL_SECS), one hour) and a
refresh token ([`REFRESH_TOKEN_TTL_SECS`](crate::REFRESH_TOKEN_TTL_SECS), seven
days); `acl:refresh` rotates the pair and kills the old one, and `acl:logout`
revokes it. Tokens are opaque random strings — the store keeps only their
sha256 digests, so the clear text exists in transit and in the client's own
session file, nowhere on the server.
