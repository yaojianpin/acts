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

Two collections in the engine's store, under prefixes of their own:

| Prefix | Row | Holds |
| --- | --- | --- |
| `acl_users` | `AclUser` | a name, salted password hashes, the command/catalog/resource/snapshot grants, an on/off flag |
| `acl_sessions` | `AclSession` | sha256 digests of a live session's access and refresh tokens, with both expiries |

The prefixes are the rows' `acts::DbCollectionIden::iden()` values, which is why
a crate outside `acts` can own a collection at all:

```rust
use acts::DbCollectionIden;

assert_eq!(acts_acl::AclUser::iden(), "acl_users");
assert_eq!(acts_acl::AclSession::iden(), "acl_sessions");
```

The rows are ordinary store documents, written through the same store the
workflow rows use (a `KvStore` backend, the database lease, the writer
shards), so a server restart keeps its users and its sessions. A session row's
id is the sha256 of its access token, and the refresh digest is held inside it,
so a refresh can be rotated exactly once; a session whose refresh token has
expired is dropped when the engine loads.

## The builtin admin

The first start of a fresh store creates `admin`
([`ADMIN_USER`](crate::ADMIN_USER)): unrestricted, with the password in
[`ADMIN_PASSWORD_ENV`](crate::ADMIN_PASSWORD_ENV) (`ACTS_ADMIN_PASSWORD`) or a
generated one printed to the log once. It cannot be deleted or disabled, so an
engine can never lock itself out of its own registry. Change its password — or
anything else about it — with `acl:setuser`.

## Users and grants

An operator writes users with the `acl:setuser` action, whose payload is an
`acts::UserSpec`: `enabled`, passwords to add and to remove by plaintext, and
the `allow`/`deny` command or catalog patterns, the `rn` patterns and the
per-target snapshot scopes. A user's grants are stored on the row and compiled
into a principal through `acts::UserPolicy` and `acts::Principal::from_policy`,
so a malformed policy is refused when the user is written rather than silently
ignored at request time. Passwords are stored salted —
`salt$sha256(salt:password)`, one entry per password, so a password can be
rotated without downtime — and a disabled user authenticates nothing,
`acl:login` included.

`acl:getuser` answers the policy without the hashes: name, `enabled`, `allow`,
`deny`, `patterns`, `snapshot`, the number of passwords and whether the user is
unrestricted.

## Sessions

`acl:login` answers an access token
([`ACCESS_TOKEN_TTL_SECS`](crate::ACCESS_TOKEN_TTL_SECS), one hour) and a
refresh token ([`REFRESH_TOKEN_TTL_SECS`](crate::REFRESH_TOKEN_TTL_SECS), seven
days); `acl:refresh` rotates the pair and kills the old one, and `acl:logout`
revokes it. Tokens are opaque random strings — the store keeps only their
sha256 digests, so the clear text exists in transit and in the client's own
session file, nowhere on the server.
