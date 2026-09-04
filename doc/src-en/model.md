# Model

The execution of the workflow engine depends on the workflow model. An acts workflow model is a standardized YAML file.

## Model Structure

A complete workflow model consists of the following parts:

```yml
id: my_model
name: Model Name

# Default variables
vars:
  - name: value
    value: 0

# Input schema (JSON Schema)
inputs:
  type: object
  properties:
    value:
      type: number

# Output schema (JSON Schema)
outputs:
  type: object
  properties:
    data:
      type: object

# Start triggers
on:
  - id: event1
    kind: manual

# Execution options
options:
  exposes:
    - name: output_key

# Step list
steps:
  - id: step1
    uses: acts.core.irq
```

## Core Concepts

| Concept | Description |
| ---- | ---- |
| [Step](./model/step.md) | The basic execution unit of a workflow, specifying a package via `uses` |
| [Branch](./model/branch.md) | Conditional branching, determining execution paths via `if` condition |
| [Act](./model/act.md) | The actual action execution body, specifying a package via `uses` |
| [Setup](./model/setup.md) | Global workflow configuration including variables, events, I/O |
| [Package](./model/pack.md) | Reusable functional modules in three categories: core, transform, event |
