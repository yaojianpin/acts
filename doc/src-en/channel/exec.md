# Execute

Execute actions on activities via client channel.

## Complete Activity

```rust
use acts::event::EventAction;

let mut outputs = Vars::new();
outputs.set("result", "done");
rt.do_action2(&pid, &tid, EventAction::Next, outputs).unwrap();
```

## Trigger Error

```rust
let mut options = Vars::new();
options.set("ecode", "err_custom");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();
```

## Back to Specific Step

```rust
let mut options = Vars::new();
options.set("to", "step1");
rt.do_action2(&pid, &tid, EventAction::Back, options).unwrap();
```

## Cancel Activity

```rust
let mut options = Vars::new();
options.set("to", "step1");
rt.do_action2(&pid, &tid, EventAction::Cancel, options).unwrap();
```

## Skip Activity

```rust
rt.do_action2(&pid, &tid, EventAction::Skip, Vars::new()).unwrap();
```

## Abort Activity

```rust
let mut options = Vars::new();
options.set("uid", "u1");
rt.do_action2(&pid, &tid, EventAction::Abort, options).unwrap();
```

## Remove Activity

```rust
rt.do_action2(&pid, &tid, EventAction::Remove, Vars::new()).unwrap();
```

## EventAction Reference

| Action | EventAction | Parameter | Description |
| ---- | ---- | ---- | ---- |
| Next | `Next` | outputs (optional) | Complete current activity, continue with output data |
| Submit | `Submit` | — | Mark activity as submitted |
| Back | `Back` | `to` — target step ID | Back to a specified step |
| Cancel | `Cancel` | `to` — target step ID | Cancel the specified step's activity |
| Skip | `Skip` | — | Skip current activity |
| Abort | `Abort` | `uid` — user identifier | Abort current activity |
| Error | `Error` | `ecode` — error code | Trigger error handling |
| Remove | `Remove` | — | Remove activity |
