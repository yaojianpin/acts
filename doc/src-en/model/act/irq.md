# IRQ

`acts.core.irq` is an interrupt request activity that pauses workflow execution and waits for a client response.

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
```

## Parameters

| Parameter | Description |
| ---- | ---- |
| key | Activity key identifier |

## Client Handling

On the client side, subscribe to `req` type messages, then complete the activity:

```rust
use acts::event::EventAction;

fn on_message(msg: &Message) {
    if msg.r#type == "req" && msg.key == "act1" {
        // Process the business logic
        let mut outputs = Vars::new();
        outputs.set("result", "processed");

        // Complete the activity
        rt.do_action2(&msg.pid, &msg.tid, EventAction::Next, outputs).unwrap();
    }
}
```

## Other Actions

In addition to `Next` (complete), IRQ activities can also be handled with other actions:

| Action | Description |
| ---- | ---- |
| `Next` | Complete the activity and pass output data |
| `Back` | Back to a specified step |
| `Skip` | Skip the activity |
| `Cancel` | Cancel the activity |
| `Abort` | Abort the activity |
| `Error` | Mark the activity as error |
| `Submit` | Submit the activity |
| `Remove` | Remove the activity |
