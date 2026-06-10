# Step Acts

A step can contain multiple activities. Activities can be combined in different ways depending on the package used.

## Single Act

The simplest case: a step uses a single act:

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
```

## Block (Nested Acts)

Use `acts.core.block` to nest multiple acts within a step. Acts execute in sequence mode by default:

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
          - uses: acts.transform.set
            params:
              a: 10
          - uses: acts.core.msg
            params:
              key: done
```

## Parallel (Loop over List)

Use `acts.core.parallel` to execute acts in parallel over a list:

```yml
steps:
    - id: step1
      vars:
        - name: users
          value:
            - u1
            - u2
      uses: acts.core.parallel
      params:
        in: '{{ users }}'
        acts:
          - uses: acts.core.irq
            params:
              key: act1
```

The engine automatically injects `index` and `value` into each child act's variable context.

## Sequence (Chain over List)

Use `acts.core.sequence` to execute acts sequentially over a list (each act waits for the previous one to complete):

```yml
steps:
    - id: step1
      vars:
        - name: users
          value:
            - u1
            - u2
      uses: acts.core.sequence
      params:
        in: '{{ users }}'
        acts:
          - uses: acts.core.irq
            params:
              key: act1
```

## Comparison

| Type | Package | Description |
| ---- | ---- | ---- |
| Parallel | `acts.core.parallel` | All child activities execute in parallel |
| Sequential | `acts.core.sequence` | Child activities execute one by one, each waiting for the previous |
| Block | `acts.core.block` | Execute nested acts in sequence mode |
