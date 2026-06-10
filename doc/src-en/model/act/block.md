# Block

Use `acts.core.block` to combine multiple activities into a block. Supports `sequence` and `parallel` execution modes.

```yml
name: test
steps:
    - id: step1
      uses: acts.core.block
      params:
        # Execution mode: sequence or parallel
        mode: sequence
        acts:
          - uses: acts.transform.set
            params:
              count: 0
          - uses: acts.core.irq
            params:
              key: act1
          - uses: acts.core.msg
            params:
              key: done
```

## Mode Comparison

| Mode | Description |
| ---- | ---- |
| `sequence` | Execute child activities one by one in order |
| `parallel` | Execute all child activities simultaneously |

## Variable Export

Child activities within a block can export variables to the parent node via `options.exposes`:

```yml
steps:
    - id: step1
      uses: acts.core.block
      params:
        mode: sequence
        acts:
          - uses: acts.core.irq
            params:
              key: act1
            options:
              exposes:
                - name: result
```
