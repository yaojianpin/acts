# Outputs

The workflow model can define an output schema that constrains the data exposed when a workflow completes.

## Exposing Outputs

Use `exposes` to filter which variables are exposed as outputs:

### Workflow Level

```yml
id: my_model
name: test
exposes:
  - name: result
  - name: data
```

### Step Level

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      exposes:
        - name: step_output
```
