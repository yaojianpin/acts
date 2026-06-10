# Execute

Execute actions on activities via client channel.

## Complete Activity

```rust
let mut options = Vars::new();
options.set("result", "done");
client.complete(&pid, &tid, options).unwrap();
```

## Trigger Error

```rust
let mut options = Vars::new();
options.set("ecode", "err_custom");
client.fail(&pid, &tid, options).unwrap();
```

## Back to Specific Step

```rust
let mut options = Vars::new();
options.set("to", "step1");
client.back(&pid, &tid, options).unwrap();
```

## Cancel Activity

```rust
let mut options = Vars::new();
options.set("to", "step1");
client.cancel(&pid, &tid, options).unwrap();
```

## Skip Activity

```rust
client.skip(&pid, &tid, Vars::new()).unwrap();
```

## Abort Activity

```rust
let mut options = Vars::new();
options.set("uid", "u1");
client.abort(&pid, &tid, EventAction::Abort, options).unwrap();
```

## Remove Activity

```rust
client.remove(&pid, &tid, EventAction::Remove, Vars::new()).unwrap();
```

