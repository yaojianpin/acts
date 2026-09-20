# Set

Use `acts.transform.set` to set variable values.

```yml
steps:
    - id: step1
      uses: acts.transform.set
      params:
        a: 10
        b: hello
        c:
          x: 1
```

## Variable Override

The value is written into the setting task's own data (a variable of the same name in that task is overridden). It does not rewrite a parent or global variable directly: it travels up in turn — when the task's `next` reaches its parent, the task's outputs are folded into the parent's data, and the parent passes them on a hop later. Reads over the chain are unchanged, so the setting task sees its own value right away.

## Set at Act Level

Set can also be used within a block:

```yml
steps:
    - id: step1
      uses: acts.core.block
      params:
        mode: sequence
        acts:
          - uses: acts.transform.set
            params:
              count: 0
          - uses: acts.core.irq
            params:
              key: act1
```
