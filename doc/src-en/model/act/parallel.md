# Parallel

Use `acts.core.parallel` to execute a collection in parallel — all child activities start simultaneously without depending on each other.

```yml
name: test
steps:
    - id: step1
      vars:
        - name: items
          value:
            - u1
            - u2
            - u3
      uses: acts.core.parallel
      params:
        in: '{{ items }}'
        acts:
          # Generates 3 IRQ activities, all executing in parallel
          - uses: acts.core.irq
            params:
              key: act1
```

## Comparison

| Type | Package | Description |
| ---- | ---- | ---- |
| Parallel | `acts.core.parallel` | All child activities execute simultaneously |
| Sequential | `acts.core.sequence` | Child activities execute one by one, each waiting for the previous |
| Block | `acts.core.block` | Execute nested acts in `sequence` or `parallel` mode |

## Variable Injection

The engine automatically injects `index` and `value` into each child activity's variable context, accessible via `{{ index }}` and `{{ value }}`.

## Dynamic Collection with Code

Combine with `acts.transform.code` to dynamically generate collections:

```yml
steps:
    - id: step1
      uses: acts.transform.code
      params: |
        let list = ["u1", "u2", "u3"];
        $set("items", list);
    - id: step2
      uses: acts.core.parallel
      params:
        in: '{{ items }}'
        acts:
          - uses: acts.core.irq
            params:
              key: act2
```
