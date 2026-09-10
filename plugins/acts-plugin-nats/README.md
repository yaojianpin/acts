# acts-plugin-nats

NATS server plugin for the acts workflow engine. It exposes the **same message
surface as `acts-plugin-grpc`** over NATS core with request/reply semantics:
remote systems send actions (including snapshot updates) and subscribe to
engine events — no gRPC needed.

## Usage

Register the plugin while building the engine. When the engine config has a
`[nats]` section, the plugin connects and starts its subscriptions:

```rust
use acts::Engine;

let engine = Engine::builder()
    .add_plugin(&acts_plugin_nats::NatsPlugin::new())
    .start()
    .await?;
```

The `acts-server` binary registers the plugin automatically as soon as its
config contains a `[nats]` section.

## Configuration

```toml
[nats]
url = "nats://127.0.0.1:4222"          # optional, default nats://127.0.0.1:4222
# token = "..."                         # token auth
# username = "u"                        # or user/password auth
# password = "p"
subject = "acts"                        # optional subject prefix, default "acts"

# one entry per remote event subscriber — the filter fields mirror the gRPC
# `OnMessage` `MessageOptions`
[[nats.channels]]
id = "ops"                              # engine delivery/ack key (default: subject)
subject = "acts.evt.ops"                # events subject (default: <prefix>.evt.<id>)
type = "*"                              # globs: message type, state, uses
state = "*"
uses = "*"
# options = { tag = "deploy" }          # custom option glob filters
```

The default subject prefix is `acts`:

- actions are received on `acts.cmd`
- a configured channel publishes its engine events on its own subject
  (default `acts.evt.<id>`)

## Actions — request/reply

A remote publishes a JSON message to the actions subject (`acts.cmd`) and
waits for the reply on its NATS request inbox:

```json
{
  "name": "snap:upsert",
  "seq": "req-1",
  "data": {
    "name": "profile",
    "scope": "u1",
    "rev": 7,
    "data": { "val": "x" }
  }
}
```

The plugin applies the action through the shared `acts::actions`
dispatch table (the same one the gRPC plugin uses — `proc:start`,
`model:deploy`, `act:*`, `msg:ack`, `evt:*`, …) and publishes the result back
to the request's reply subject:

```json
{ "name": "snap:upsert", "ack": "req-1", "data": true }
```

Failures reply with `err` filled (`data` is `null`); a message without a
reply subject is still executed but produces no reply.

```rust,ignore
let client = async_nats::connect("nats://127.0.0.1:4222").await?;

let cmd = serde_json::json!({
    "name": "snap:upsert",
    "seq": "req-1",
    "data": {
        "name": "profile",
        "scope": "u1",
        "rev": 7,
        "data": { "val": "x" }
    }
});
let reply = client.request("acts.cmd", cmd.to_string().into()).await?;
let resp: serde_json::Value = serde_json::from_slice(&reply.payload)?;
assert_eq!(resp["data"], true);
```

Snapshot deletion uses `snap:remove` with `{ name, scope }`.

## Events

Each configured channel subscribes to engine messages matching its filters
and forwards every one to its NATS subject with the envelope

```json
{
  "name": "<message type>",
  "seq": "<message/delivery id>",
  "data": { "...full message payload, e.g. type/state/pid/tid/uses..." }
}
```

Ack semantics equal the gRPC flow: the engine stores each delivery for the
channel and its retry timer re-sends deliveries that were not acked. A remote
that handled a message acks it with the standard `msg:ack` action:

```json
{ "name": "msg:ack", "seq": "ack-1", "data": { "id": "<seq of the event>" } }
```

Sending the ack stops redelivery of that event.

## Tests

The live tests (`tests/live.rs`, plus the acts-server wiring test) need a
running NATS server. They read the url from the `ACTS_NATS_URL` environment
variable (default `nats://127.0.0.1:4222`) and **skip automatically** when no
server is reachable, so plain `cargo test` stays green without NATS.

```bash
cargo test -p acts-plugin-nats --test live
cargo test -p acts-server --test nats
```

In GitHub Actions the NATS server is provided as a service container
(`nats:2`) with `ACTS_NATS_URL=nats://127.0.0.1:4222`, so the same tests run
against a real broker in CI.
