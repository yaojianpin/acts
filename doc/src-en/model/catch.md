# Catches

The error handling mechanism allows recovery when a step encounters an error.

## Step-Level Catches

Steps define error handling via `catches`, which is a list of `Step` objects:

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      catches:
        # Match specific error code
        - uses: acts.core.msg
          if: $ecode() == 'err1'
          params:
            key: catch_err1

        # Match all unhandled errors
        - uses: acts.core.msg
          params:
            key: catch_others
```

## Error Handling Flow

1. An activity in a step triggers an error (via `EventAction::Error` or `acts.core.action`)
2. The engine checks the `catches` list conditions in order
3. Executes the corresponding handler when the first matching catch is found
4. After handling, the step continues normal execution
5. If no catch matches, the error propagates upward

## Error Code

Error codes are passed via `ecode`:

```rust
let mut options = Vars::new();
options.set("ecode", "err1");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();
```

Use `$ecode()` in the catch `if` condition to get the error code.
