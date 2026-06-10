# Sequence

Use `acts.core.sequence` to execute sequential chains over a collection, where each subsequent execution depends on the completion of the previous one.

```yml
name: test
steps:
    - id: step1
      vars:
        - name: items
          value:
            - u1
            - u2
      uses: acts.core.sequence
      params:
        in: '{{ items }}'
        acts:
          # Generates 2 IRQ activities, executed one by one in sequence
          - uses: acts.core.irq
            params:
              key: act1
```

## Comparison

| Type | Package | Description |
| ---- | ---- | ---- |
| Parallel | `acts.core.parallel` | All child activities execute simultaneously in parallel |
| Sequential | `acts.core.sequence` | Child activities execute one by one in order, each waiting for the previous |
| Block | `acts.core.block` | Execute nested acts in `mode: sequence` order |

The engine automatically injects `index` and `value` into each child activity's variable context.
