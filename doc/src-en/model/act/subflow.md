# Subflow

Use `acts.core.subflow` to invoke another workflow model (sub-workflow).

```yml
name: test
steps:
    - id: step1
      uses: acts.core.subflow
      params:
        # The target sub-workflow model ID
        to: sub_workflow_id
        # Input data passed to the sub-workflow
        a: '${{ value }}'
```

## Sub-Workflow Definition

A sub-workflow is an independent workflow model:

```yml
id: sub_workflow_id
name: sub_flow
inputs:
  type: object
  properties:
    a:
      type: integer
outputs:
  type: object
  properties:
    result:
      type: string
steps:
    - id: sub_step1
      uses: acts.core.irq
      params:
        key: sub_act

    - id: sub_step2
      uses: acts.core.msg
      params:
        key: sub_done
```

## Data Passing

Sub-workflow inputs are passed via `params`, and sub-workflow outputs are exported back to the parent workflow via `options.exposes`:

```yml
steps:
    - id: step1
      uses: acts.core.subflow
      params:
        to: sub_workflow_id
        input_value: '${{ parent_var }}'
      options:
        exposes:
          - name: result
```
