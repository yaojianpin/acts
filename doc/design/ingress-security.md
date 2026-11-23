# Ingress Security Architecture (Volume-8 Hardening)

Design for the eight audit findings NET-WEB/GRPC/SUB/NATS/TLS, ERROR-01, LOG-01,
FS-01/02, STATUS-01 and DEPLOY-01, verified against this repository on the
`develop` branch (verification matrix in
[`docs/plan/hardening-p1-p4.md`](../plan/hardening-p1-p4.md)).

## 1. Goal

A request must never cost the engine more than its principal and its action
deserve, at any stage: not in raw bytes before the size check, not in parse
work before authentication, not in queue allocations before a quota is
checked, not in log volume after a refusal. Secrets leave the process only
through a redacted face.

## 2. The one shared budget: `acts::Ingress`

New core module `crates/acts/src/ingress.rs`. One instance per engine,
built from config at `Engine` construction; every transport talks to
`engine.ingress()`.

```text
Profile        per-action {max_raw_bytes, deadline_ms, max_inflight}
try_enter_action   global + per-action inflight permits (try-acquire)
try_enter_parse    parse-pressure permits (try-acquire)
subscriptions      cross-transport SubscriptionRegistry (leases)
metrics            acts_ingress_* counters + acts_subscriptions gauges
```

### 2.0 Posture levels (`[security].level`, per-section override)

One knob presets every budget so a deployment says its posture once and
still keeps every fine-grained key. Resolution: **explicit key > section
`level` > `[security].level` > `standard`**. An unknown level word warns
and falls back to `standard`.

| | `permissive` | `standard` (default) | `strict` |
|---|---|---|---|
| payload caps (§2.1) | ×4 | as documented | ÷4 (16 KiB floor) |
| per-action inflight | ×2 | as documented | ÷2 |
| `[ingress] global_inflight` | 4096 | 1024 | 512 |
| `[ingress] parse_inflight` | 256 | 128 | 64 |
| `[subscriptions]` global/subject/ip/rate | 20 000 / 64 / 256 / 40 | 10 000 / 32 / 128 / 20 | 2 000 / 16 / 64 / 10 |
| web/gRPC non-loopback plaintext | warn only | refused without `tls` or `plain_behind_proxy` | refused without `tls` — `plain_behind_proxy` is not enough |
| `[server] fs_strict` default | false | false | **true** |

Core API: `acts::Level` (`Permissive`/`Standard`/`Strict`),
`acts::resolve_level(section_level, &config)`, `engine.ingress().level()`.

### 2.1 Profiles

Built-in classes (all override-able per class or per exact action name via
`[ingress.profile.<key>]`; unknown keys in `[ingress.profile]` are an error):

| class | actions | max_raw_bytes | deadline | max_inflight |
|---|---|---:|---:|---:|
| `auth` | `acl:login`, `acl:refresh` | 64 KiB | 10 s | 32 |
| `small` | row reads, acks, `msg:sub`, ACL reads | 64 KiB | 5 s | 128 |
| `event` | `evt:start` (and the `/hooks` route) | 1 MiB | 15 s | 64 |
| `snapshot` | `snap:upsert/remove/reply` | 1 MiB | 15 s | 32 |
| `start` | `proc:start`, `proc:start_from_model` | 2 MiB | 30 s | 32 |
| `deploy` | `model:deploy`, `pack:publish` | 8 MiB | 60 s | 4 |
| `default` | everything else | 256 KiB | 30 s | 64 |

Clamps: bytes 16 KiB..256 MiB, deadline 1 s..1 h, inflight 1..65 536, global
inflight default 1024, parse permits default 128 (`[ingress]` section).
`0` selects the default (mirroring `[nats].max_in_flight`).

### 2.2 Admission order (every transport)

```text
1 raw size vs hard ceiling            → reject before any parse
2 parse permit                        → reject on parse pressure
3 minimal envelope / metadata parse
4 authenticate (token → principal)
5 profile size check (raw vs profile) → reject oversize for the action
6 action permits (global + per-action)
7 full data decode (within profile)
8 dispatch under the profile deadline
```

Permits are try-acquire. A refusal is answered to the caller with the stable
code `resource_exhausted` (or `busy` for parse pressure) and counted under
`acts_ingress_rejected_total{reason}` — reasons: `global_inflight`,
`action_inflight`, `parse_pressure`, plus per-transport `payload_size`,
`rate_limited`.

### 2.3 Subscription registry

`[subscriptions]`: `global_max = 10 000`, `per_subject = 32`,
`per_ip = 128`, `per_client = 1`, `create_rate_per_sec = 20`.

SSE and gRPC acquire a lease (`SubscriptionKey { transport, subject,
client_id, ip }`) **before** allocating their queue and registering the
channel handler; the lease's `Drop` releases every bucket. The same
`(subject, client_id)` refuses a second connection (`ClientBusy`) — two
connections must never ack the same delivery. An empty client id cannot
claim the per-client bucket (nothing stable to dedupe on) and is counted in
global/subject/IP only. Gauges: `acts_subscriptions{transport}` and
`acts_subscriptions_total`.

## 3. The one error contract: `acts::export::error`

`ExternalError { code, message, correlation_id, retryable }`; codes:
`invalid_argument`, `not_found`, `unauthenticated`, `permission_denied`,
`conflict`, `resource_exhausted`, `unavailable`, `internal`.

- `internal` failures answer the fixed phrase `"internal error; quote the
  correlation id to the operator"`; the full detail (store path, SQL, stderr)
  is logged server-side under the same correlation id. All other kinds keep
  their caller-facing text, run through `safe_text` (control characters
  neutralized, length bounded, `…` at a char boundary).
- `actions::Error` gained `ResourceExhausted` (queue/parse/admission load)
  and `Unavailable` (shutdown, lease lost); `ActError::QueueFull/Shutdown/
  LeaseLost` map onto them in `value()`/`acl_error()`, so load and shutdown
  are no longer `internal`.
- Wire mapping: HTTP status per code (400/404/401/403/409/429/503/500) with
  `x-correlation-id` response header; gRPC status code + the JSON contract in
  the status raw details; NATS reply carries `err` (prose, for old clients)
  plus `ecode`/`correlation_id`/`retryable`.
- Correlation ids: `new_correlation_id()` (16 url-safe chars) per request,
  threaded through logs, spans and every error face.

## 4. Per-transport design

### 4.1 Web (`acts-plugin-web`)

- Per-route body caps from the profile (`axum::extract::DefaultBodyLimit`),
  profile chosen by route: deploy→8 MiB, start→2 MiB, hooks/snap→1 MiB,
  small/ack→64 KiB, health→exempt.
- Middleware stack (axum `from_fn`, no tower-http):
  global concurrency gate (`[web].max_concurrent`, default 512, try-acquire
  → 429 + `Retry-After`), per-IP concurrency (`[web].max_concurrent_per_ip`,
  default 64), per-IP token-bucket rate limit (`[web].rate_limit_per_min`,
  default 600, `0` = off), request timeout (profile deadline → 503
  `deadline_exceeded`, correlation id included), quiesce gate (503 once
  draining), correlation middleware (`x-correlation-id` in/out + tracing
  span `ingress{transport,action,cid}`), metrics outermost.
- Client IP: `ConnectInfo<SocketAddr>`; `X-Forwarded-For` honored only when
  the direct peer is in `[web].trusted_proxy_cidrs` — the last untrusted
  entry is the client. Proxies must not log `Authorization`; TLS (or a
  protected internal network) stays required between proxy and acts.
- Auth semantics (unchanged, verified): `MissingCredentials` → anonymous
  caller (refused per action), `InvalidCredentials` → immediate 401 before
  the body is read.
- TLS: `[web].tls = { cert, key, client_ca? }` wraps the listener in a
  rustls acceptor (TLS 1.2/1.3, modern defaults; `client_ca` enables
  mutual TLS). Strict mode: a non-loopback bind with neither TLS nor
  `[web].plain_behind_proxy = true` **refuses to start**.
- SSE: subscription lease before the channel; lease guard lives inside the
  response stream, so a disconnect releases the quota slots.
- Graceful stop: registers a `RunningTransport` (web) at `start`; its
  `InflightCounter` is the concurrency gate — quiesce flips it, drain waits
  it out; `Engine::close` drives both before plugin shutdown.

### 4.2 gRPC (`acts-plugin-grpc`)

- `ActsServiceServer` caps: `max_decoding_message_size = 8 MiB`,
  `max_encoding_message_size = 16 MiB` (explicit contract, not framework
  defaults).
- Field validation before any allocation from user input: `client_id` ≤
  256 B, `name`/`seq` ≤ 256 B, `ack` ≤ 1 KiB, `options` ≤ 64 entries with
  keys ≤ 128 B and values ≤ 1 KiB → `invalid_argument`.
- Admission order per §2.2: the bearer token resolves **before** the `data`
  JSON decode; the decode holds a parse permit; the dispatch holds the
  action permits and runs under `min(client timeout, profile deadline)`
  (`tonic::Request::timeout` respected when the client sends one).
- Subscriptions: lease per `on_message` with the peer IP and the `msg:sub`
  client id; over quota answers `resource_exhausted` (details carry the
  code). Unary concurrency bounded by the same global/action permits.
- HTTP/2 keepalive: server ping every 30 s, ping timeout 15 s — half-dead
  connections die instead of holding stream slots.
- TLS: `[grpc].tls` identical shape; strict mode identical rule
  (`[grpc].plain_behind_proxy` override).
- Graceful stop: `RunningTransport` (grpc) counts unary + live streams;
  quiesce closes the listener half (streams keep flowing), drain waits.

### 4.3 NATS (`acts-plugin-nats`)

- `[nats].max_payload_bytes` (default 1 MiB, deploy actions still capped by
  the broker's own `max_payload`): checked **before** `serde_json` sees the
  bytes.
- `WireMessage.data` becomes a `serde_json::value::RawValue` — the envelope
  (`name`, `seq`, `ack`, `token`) is parsed first; the data is decoded only
  after authentication and after the action permits are held, inside the
  profile budget.
- Parse permits (`engine.ingress().try_enter_parse()`) held across the
  envelope parse; the action permits across the data decode + dispatch,
  under the profile deadline.
- Rejections (size, parse pressure, admission, auth) reply with the error
  contract fields (§3); reasons are counted, payloads never logged (only
  `payload_bytes`).
- `[nats].tls_required = true` makes `ConnectOptions::tls_required` insist
  on broker TLS — the payload token must not cross a plaintext wire.
- `[nats].reject_jetstream_capture = true` checks (via the JetStream
  management API) that no stream's subjects overlap `<prefix>.cmd` before
  subscribing; overlap is a start failure when strict, a warning otherwise.
  Deploy requirements for the payload-token design are documented in the
  plan (§ NET-NATS-02).

### 4.4 Server (`acts-server`) — filesystem hardening

- Startup (before any store opens) sets a safe umask on unix (`0o077`) so
  every created file/dir — including Rolling-appender hours and RocksDB
  WAL/SST children — lands `0600`/`0700` regardless of the caller's umask.
  On windows, created config/log/store directories get an exclusive DACL
  (service account + SYSTEM + Administrators).
- `fs_hardening` module: `ensure_dir_secure` / `ensure_file_secure` (create
  with the explicit mode immediately after create — no check-then-create
  windows on the file itself), symlink rejection on the final component and
  every intermediate (`symlink_metadata` checks), group/other-write and
  world-writable-parent rejection.
- `[server] fs_strict = true` (default false): violations refuse the start;
  the default logs a warning with the path and fixes what fixing can fix.
- Applied to: config dir + `acts.toml`, log dir, store dir (`ensure_store_dir`),
  data dir; `acts-cli` session file keeps its existing `0600`.

### 4.5 `sys:status` redaction and budget

- `config` echo passes `redact_config`: sensitive-name fields (`password`,
  `token`, `secret`, `credential`, `api_key`, `private_key`, …) answer
  `"[configured]"`; URL credentials scrub (`postgres://user:***@host/db`,
  query `password=` too); **unclassified fields and sections answer
  `"[redacted]"`** — default-deny for future keys; allow-listed sections
  (`db`, `web`, `grpc`, `nats`, `log`) publish their operational fields;
  `[ingress]`/`[subscriptions]`/`[shutdown]` pass through (pure knobs).
- Budget: 2 s cache (per engine), 4 concurrent samples max (overruns answer
  the cache), 5 s hard timeout. `metrics` field added to `SystemStatus`
  (mirrored in `acts-channel`).

## 5. Lifecycle (P4)

- `RunningTransport { transport(), quiesce(), drain(deadline) }` in core;
  transports register at `start` via `Engine::register_transport`.
- `Engine::close`: cancel token → quiesce each transport → drain each
  (`[shutdown].drain_timeout_ms`, default 5 s, clamp 100 ms..60 s, report
  logged) → plugin `shutdown` in reverse order (unchanged).
- Transports keep refusing after quiesce: web 503 `unavailable`, gRPC
  `UNAVAILABLE`, NATS replies the same; admitted work finishes or the report
  names what remained.

## 6. What is deliberately *not* here

- NKeys/account-JWT identity for NATS (replacing the payload token) is an
  architecture change with broker-side deployment work; the plan tracks it
  as a deployment requirement, not code.
- `openat2(RESOLVE_BENEATH)` is Linux-only; the portable symlink/mode/owner
  validation here is check-at-startup plus explicit modes, not a
  runtime-resolved dirfd. Container deployments get their hardening from
  the read-only-root pattern, documented in the plan.
- Prometheus/scrape endpoints: metrics live in `sys:status` (`metrics`
  map); a push/export integration is left to the deployer.

## 7. Acceptance (from the audit, mapped)

| audit criterion | where |
|---|---|
| Chunked/oversize bodies refused before full read | web `DefaultBodyLimit` per route |
| gzip bombs never reach serde | web does not decode compressed bodies (no compression layer exists); a future one must add a decompression budget before `Json` |
| Concurrency caps do not grow the scheduler queue | permits are try-acquire; refusals precede `apply_as` |
| Deploy vs normal actions differ | profile classes, §2.1 |
| Decode-size refusals precede Prost allocation | gRPC `max_decoding_message_size` + raw-size check |
| Same `(subject, client_id)` cannot double-ack | registry `ClientBusy` |
| SSE + gRPC share per-subject caps | one registry |
| NATS oversize never enters serde; parse has its own semaphore | §4.3 order |
| Canary secrets absent from status | `redact_config` tests |
| One error text on all three transports | `ExternalError` mapping |
| Non-loopback without TLS refuses to start | strict mode, §4.1/4.2 |
| Files safe under `umask 000` | startup umask + explicit modes |
| Symlinked config/log/store paths rejected | `fs_hardening` checks |
| Status cache absorbs 100 concurrent calls | gate + cache, §4.5 |
