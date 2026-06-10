# MSG

`acts.core.msg` is a one-way message activity that sends a notification to the client without pausing workflow execution.

```yml
steps:
    - id: step1
      uses: acts.core.msg
      params:
        key: notification
```

## Parameters

| Parameter | Description |
| ---- | ---- |
| key | Message key identifier |

## Difference from IRQ

| Feature | IRQ | MSG |
| ---- | ---- | ---- |
| Pauses execution | Yes | No |
| Waits for response | Yes | No |
| Client must respond | Yes (Next/Error/...) | No |

## Client Reception

On the client side, subscribe to `msg` type messages to receive notifications:

```rust
fn on_message(msg: &Message) {
    if msg.r#type == "msg" {
        match msg.key.as_str() {
            "notification" => {
                println!("Received notification: {:?}", msg);
                // No need to call do_action to respond
            }
            _ => {}
        }
    }
}
```
