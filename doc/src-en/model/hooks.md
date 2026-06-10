# Events

Workflow events are defined via the `on` field to trigger workflow startup.

## Event Types

| Package | Type | Description |
| ---- | ---- | ---- |
| `acts.event.manual` | Manual Event | Synchronous manual event trigger |
| `acts.event.hook` | Hook Event | Hook event that waits for completion |
| `acts.event.chat` | Chat Event | Chat event |

## Manual Event

```yml
name: test
on:
  - id: event1
    uses: acts.event.manual
steps:
  - id: step1
    uses: acts.core.irq
```

Triggered in code:

```rust
let mut vars = Vars::new();
vars.set("data", "event_data");
executor.evt().start("model-id:event1", &vars)?;
```

## Hook Event

```yml
name: test
on:
  - id: event1
    uses: acts.event.hook
steps:
  - id: step1
    uses: acts.core.irq
```

## Chat Event

```yml
name: test
on:
  - id: event1
    uses: acts.event.chat
steps:
  - id: step1
    uses: acts.core.irq
```
