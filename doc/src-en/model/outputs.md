# Outputs

The workflow model can define an output schema that constrains the data exposed when a workflow completes.

## Output Schema

Uses JSON Schema format to define the output schema:

```yml
id: my_model
name: test
outputs:
  type: object
  properties:
    result:
      type: string
      description: The result of the workflow
    data:
      type: object
      description: The output data
steps:
    - id: step1
      uses: acts.transform.set
      params:
        result: done
        data:
          count: 100
```

## Exposing Outputs

Use `options.exposes` to filter which variables are exposed as outputs:

### Workflow Level

```yml
id: my_model
name: test
options:
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
      options:
        exposes:
          - name: step_output
```
