# Step Setup

Steps can define local variables and export options.

## Local Variables

Use `vars` to define local variables for a step:

```yml
steps:
    - id: step1
      vars:
        - name: count
          value: 0
        - name: list
          value:
            - u1
            - u2
      uses: acts.core.irq
      params:
        key: act1
```

## Exporting Outputs

Use `options.exposes` to export step-level output variables:

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      options:
        exposes:
          - name: result
```

The exported `result` variable will be available to subsequent steps.
