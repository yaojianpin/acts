# Ingress Hardening Plan — P1–P4 (Volume-8 findings)

Status file for the P1–P4 remediation of the volume-8 audit report
(`docs/design/ingress-security.md` is the architecture). Every phase below is
implemented **together** on one branch and verified by one unified test pass.

## 0. Verification matrix (audit → code, before the work)

| finding | verdict | evidence |
|---|---|---|
| NET-WEB-01 no body/concurrency/timeout/load-shed | confirmed | `plugins/acts-plugin-web/src/lib.rs` — bare `Router` + `axum::serve`, zero tower layers; only axum's implicit 2 MiB JSON cap |
| NET-WEB-02 three-state auth mapping | partially stale | `auth.rs` already maps Missing→anonymous-refused-per-action / Invalid→401; missing: duplicate-header rejection, header-length bound, distinguishing metrics |
| NET-GRPC-01 no explicit message caps | confirmed | `Server::builder()` without decode/encode caps (tonic defaults: decode 4 MiB, encode unlimited); no field-level bounds |
| NET-GRPC-02 no quotas/deadline/keepalive | confirmed | no unary/stream caps, no server deadline, no HTTP/2 keepalive |
| NET-SUB-01 cross-transport stacking | confirmed | `msg:sub` computes a key and registers a channel with no count anywhere |
| NET-NATS-01 parse before permit | confirmed | `serve_actions`: `serde_json::from_slice::<WireMessage>` (with `data: JsonValue`) before `ActionsInFlight::try_enter`; no raw-size check |
| NET-NATS-02 token in payload | confirmed (documented design) | broker authenticates the connection; per-request identity can only come from the payload; no TLS/JetStream checks |
| NET-TLS-01 non-loopback without TLS | confirmed (conditional) | loopback defaults; no TLS support; non-loopback bind silently plaintext |
| ERROR-01 internal error echo | confirmed | `From<ActError> → to_string()` (500), gRPC `Status::internal(msg)`, NATS `err: to_string()`; no stable codes, no correlation ids |
| LOG-01 no sanitization/redaction | confirmed | no redact/sanitize anywhere; user-controlled fields Debug-printed; no Secret types |
| FS-01 umask-dependent modes | confirmed | `create_dir_all`/`fs::write`/Rolling appender/store dir — no mode anywhere except acts-cli session 0600 |
| FS-02 symlink/TOCTOU | confirmed | `path.exists()` → create patterns; no O_NOFOLLOW/openat |
| STATUS-01 full config echo | confirmed | `collect()`: `config = to_value(config.table)` verbatim; unbounded concurrent samples (new `System` + double CPU sample per call) |
| DEPLOY-01 `on_init` starts listener | stale | all three plugins already prepare/start/shutdown; listener bound in `start`, broker connect in `prepare`, tasks joined on shutdown; only quiesce/drain/TLS-loading and a unified handle are missing |

Two findings the audit missed (found during verification):

1. **gRPC parses `data` before authentication** (`do_action` decodes
   `Vars` before `acl().resolve`) — the NATS issue one level in.
2. **Web anonymous requests pay the full body parse before the per-action
   401** (middleware admits the anonymous principal; the `Json` extractor
   consumes the body first). Invalid credentials are refused pre-parse.

## 1. Phase plan

### P1 — immediate boundaries (all: **done, this branch**)

| # | item | where | status |
|---|---|---|---|
| 1 | Web route body limit / timeout / concurrency / load-shed / per-IP rate / trusted proxy | `plugins/acts-plugin-web/src/{lib,limit,config}.rs` | done |
| 2 | gRPC encode/decode caps + field bounds + stream/unary quotas + deadline + keepalive | `plugins/acts-plugin-grpc/src/lib.rs` | done |
| 3 | NATS raw-size check + parse semaphore + envelope-first + `RawValue` data | `plugins/acts-plugin-nats/src/lib.rs` | done |
| 4 | `sys:status` redaction + cache/concurrency/timeout + metrics | `crates/acts/src/export/status.rs` | done |
| 5 | External error DTO + secret-safe internal handling + correlation ids | `crates/acts/src/export/error.rs` + three transports | done |
| 6 | Explicit modes for config/data/log (umask 0o077 startup, unix modes, windows DACL) | `crates/acts-server/src/fs_hardening.rs` | done |

### P2 — unified admission (all: **done, this branch**)

| # | item | where | status |
|---|---|---|---|
| 1 | Action ingress profiles | `acts::ingress::Profile` + `[ingress]` | done |
| 2 | Shared budget across web/gRPC/NATS | `engine.ingress()` (one instance) | done |
| 3 | Subscription registry | `SubscriptionRegistry` + leases in SSE/gRPC | done |
| 4 | Trusted proxy + per-IP rate limit (web) | `[web].trusted_proxy_cidrs` + token bucket | done |
| 5 | Parse / auth(KDF) / action semaphores | `try_enter_parse` + `auth` class cap + `try_enter_action` | done |

### P3 — TLS and path hardening (all: **done, this branch**)

| # | item | where | status |
|---|---|---|---|
| 1 | Strict TLS for non-loopback binds | `[web].tls` / `[grpc].tls` + `plain_behind_proxy` escape | done |
| 2 | Mutual TLS | `tls.client_ca` on both transports | done |
| 3 | NATS broker checks | `[nats].tls_required`, `[nats].reject_jetstream_capture` | done |
| 4 | Symlink / owner / mode validation of roots | `fs_hardening` checks at startup | done |
| 5 | Windows DACL | `fs_hardening::windows` (exclusive DACL on created dirs) | done |

`openat2`-level runtime confinement is out of scope (Linux-only); see design
§6 for the boundary and the container pattern.

### P4 — operations (all: **done, this branch**)

| # | item | where | status |
|---|---|---|---|
| 1 | `RunningTransport` handles | `acts::plugin::transport` + `Engine::register_transport` | done |
| 2 | Quiesce + drain with reports | `Engine::close` + `InflightCounter` per transport | done |
| 3 | Correlation ids end to end | `new_correlation_id` in every transport entry + error faces | done |
| 4 | Metrics + status gauges | `acts::ingress::Metrics` + `SystemStatus.metrics` | done |
| 5 | Failure-injection tests | per-crate suites (below); the 10 k-stream soak is `#[ignore]`d scale work | done (soak = ignored) |

## 2. Unified test pass (after all phases)

Gate order (repo rules, from the repository root):

```sh
cargo fmt
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo test -p acts --lib
cargo test -p acts --doc
cargo test -p acts-acl --lib
cargo test -p acts-expr --lib --all-features
cargo test -p acts-channel --lib
cargo test -p acts-plugin-web --lib
cargo test -p acts-plugin-grpc --lib
cargo test -p acts-plugin-nats --lib
cargo test -p acts-package-http --lib   # etc. per repo list
cargo test -p acts-store --lib --features sled,sqlite,postgres
cargo test -p acts-cli --lib
cargo test -p acts-server --lib
cargo deny --locked check
cargo audit --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2026-0459 --ignore RUSTSEC-2025-0057 --ignore RUSTSEC-2024-0384
```

Environment dependencies (stated plainly per repo rules):

- `acts-store`'s postgres suite needs Postgres 16 on `localhost:5433`;
  `acts-channel`/`acts-plugin-nats`/`acts-server` broker-backed suites need
  NATS+JetStream on `127.0.0.1:4222`. This workstation runs **neither**, so
  those specific integration cases are excluded from the local "green"
  claim and marked in the run report; CI starts both.
- The unix-only filesystem cases (umask, symlink races, mode assertions)
  are `#[cfg(unix)]`: they cannot execute on this windows host. They run in
  CI (ubuntu); the windows DACL cases run here.
- The 10 k-stream soak is `#[ignore]` (CI-budget scale work); the quota
  semantics are covered at CI scale (hundreds of connections).

## 3. Coverage target (95 %+ on the changed surface)

Measured with `cargo llvm-cov` restricted to the crates the phases touched:
`acts` (ingress/export::error/export::status/plugin::transport/engine), the
three plugins, `acts-server` (fs_hardening). The uncovered remainder —
tls-handshake plumbing against real certs, broker round-trips without a
broker, OS-DACL error branches — is enumerated in the run report rather
than padded with tautological tests.

## 4. Deployment requirements carried by design (not code)

- **NET-NATS-02**: while the access token travels in the action payload,
  production requires: TLS between every publisher and broker
  (`[nats].tls_required = true` enforces it client-side), no JetStream
  stream capturing `<prefix>.cmd` (`[nats].reject_jetstream_capture`),
  subject ACLs that allow the server only subscribe + reply-publish,
  short-lived access tokens only (never refresh tokens), and no payload
  logging anywhere (enforced: only sizes and codes are logged).
- **Trusted proxy**: `[web].trusted_proxy_cidrs` must list exactly the
  proxies that may set `X-Forwarded-For`; the proxy must not log
  `Authorization`; the proxy→acts hop uses TLS or a protected network;
  direct access to the backend port is firewalled off.
- **Azure App Gateway / NGINX / Envoy** timeouts must exceed the transport
  deadlines (§ design 2.1) so the server, not the proxy, ends a request.

## 5. Verification record (this branch)

Filled by the unified pass; numbers come from the logs, not from intent.

| suite | result |
|---|---|
| `acts` lib (core) | 1887 passed / 0 failed |
| `acts-plugin-web` lib | 61 passed / 0 failed |
| `acts-plugin-grpc` lib | 41 passed / 0 failed |
| `acts-plugin-nats` lib | 21 passed / 0 failed |
| `acts-server` (lib + tests/) | 24 lib + 5 main + 5 fs_hardening + 2 lease + 1 nats + 3 startup = 40 passed / 0 failed |
| `acts-channel` / `acts-cli` | 86 + 31 passed / 0 failed |
| `acts-acl` / package libs (http, shell, state, nats, javascript) | 32 + 33 + 10 + 14 + 5 + 28 passed / 0 failed |
| `acts-expr --all-features` / `acts-store` (sled,sqlite,postgres) / `acts --doc` | 85 / 7 / 12 passed / 0 failed |
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features --workspace --locked -- -D warnings` | clean (0 warnings) |
| coverage (`cargo llvm-cov`, changed crates) | 95.37% lines / 96.38% regions (acts + web); NATS lib 83.16% / gRPC lib 81.14% after their broker/TLS additions |
| `cargo deny --offline --locked check` | advisories ok, bans ok, licenses ok, sources ok |
| `cargo audit -n --stale` (repo ignores) | clean |

Build-environment note: this workstation intermittently reported "invalid
metadata files" for freshly written rlibs under parallel builds (a
defender/disk-race symptom, reproducible before this branch's changes). The
unified pass therefore ran with `CARGO_INCREMENTAL=0` and `-j 2` after a
full `cargo clean`; every result in the table above comes from that clean
run. No source workaround is part of this change.

Platform notes: this workstation runs windows; the unix-only filesystem cases
(`#[cfg(unix)]` in `crates/acts-server/tests/fs_hardening.rs`) compile here
and run in CI. The windows DACL cases run here. Postgres-backed store cases
and broker-backed channel cases need the services named in §2 and were not
available locally; that is stated in the hand-off rather than claimed green.

## 6. Coverage detail (changed surface)

`cargo llvm-cov --summary-only -j 2 --lib`, windows host, unified tests:

| file | lines | regions |
|---|---:|---:|
| `crates/acts/src/ingress.rs` | **97.82%** | 97.78% |
| `crates/acts/src/export/error.rs` | **98.70%** | 99.12% |
| `crates/acts/src/export/status.rs` | 90.29% | 87.50% |
| `crates/acts/src/plugin/transport.rs` | **97.32%** | 96.20% |
| `crates/acts/src/engine.rs` | 76.63% | 76.25% (pre-existing scheduler code dominates; the new close/drain paths are covered by two tests) |
| `plugins/acts-plugin-web/src/{auth,config,objects,limit,sse}.rs` | **97.1–100%** | 96.6–100% |
| `plugins/acts-plugin-web/src/{lib,routes}.rs` | 82.4% / 92.4% | 81.8% / 94.4% |
| `plugins/acts-plugin-grpc/src/lib.rs` | 81.14% | 80.34% (was 74.62 / 73.53) |
| `plugins/acts-plugin-nats/src/lib.rs` | 83.16% | 83.07% (was 49.48 / 48.23) |
| `crates/acts-server/src/fs_hardening.rs` | 81.33% | 81.25% (unix branches not compiled here; windows symlink creation needs a privilege the test account lacks) |

Total for the `acts` + web measurements: **95.37% lines, 96.38% regions**, 93.91% functions.
The remaining uncovered branches are named rather than padded: `sys:status` sample-timeout/overload paths (need a wedged store or static gate injection), the web TLS accept loop's shutdown legs, gRPC's mutual-TLS success path (needs a CA-constrained certificate), NATS connect/keepalive failure legs, and the OS-privilege-only filesystem branches.
