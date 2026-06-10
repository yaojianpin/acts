# Step Catches

When a step encounters an error, use `catches` to define error handling logic.

## Basic Usage

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      catches:
        - uses: acts.core.msg
          if: $ecode() == 'err1'
          params:
            key: catch_err1

        - uses: acts.core.msg
          params:
            key: catch_others
```

## Matching All Errors

If no `if` condition is specified, the catch matches all errors:

```yml
catches:
    - uses: acts.core.msg
      params:
        key: catch_all
```

## Triggering Errors

Errors are triggered on the client side via `EventAction::Error`:

```rust
let mut options = Vars::new();
options.set("ecode", "err1");
rt.do_action2(&pid, &tid, EventAction::Error, options).unwrap();
```

Or via `acts.core.action` in timeouts:

```yml
timeouts:
    - uses: acts.core.action
      if: $cost_in('8s')
      params:
        action: error
        options:
          ecode: err_timeout
```

## Catch Execution Order

1. The catcher checks conditions from top to bottom
2. The first catch with a matching condition is executed
3. After the catch completes, the step continues normal execution
4. If no catch matches, the error propagates upward
