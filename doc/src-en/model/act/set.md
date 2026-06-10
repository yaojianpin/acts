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

If the setting variables already exist in the current context, they will be overridden.

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
