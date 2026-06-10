# Action

Use `acts.core.action` to execute engine commands, such as triggering errors, completing steps, etc.

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      timeouts:
        # Trigger error on timeout
        - uses: acts.core.action
          if: $cost_in('8s')
          params:
            action: error
            options:
              ecode: err_timeout
```

## Supported Commands

| Command | Description |
| ---- | ---- |
| `error` | Trigger an error, can pass `ecode` to specify error code |

## Client Commands

The client can also use `do_action2` to perform the following operations to affect activity state:

| Operation | EventAction | Description |
| ---- | ---- | ---- |
| Complete | `Next` | Complete current activity, continue to next step |
| Submit | `Submit` | Submit the current activity |
| Back | `Back` | Back to a specified step |
| Cancel | `Cancel` | Cancel a specified activity |
| Skip | `Skip` | Skip current activity |
| Abort | `Abort` | Abort current activity |
| Error | `Error` | Mark activity as error |
| Remove | `Remove` | Remove activity |

```rust
// Complete activity
rt.do_action2(&pid, &tid, EventAction::Next, Vars::new()).unwrap();

// Trigger error
let mut options = Vars::new();
options.set("ecode", "err1");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();

// Back to specified step
let mut options = Vars::new();
options.set("to", "step1");
rt.do_action2(&pid, &tid, EventAction::Back, options).unwrap();
```
