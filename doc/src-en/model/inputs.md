# Inputs

The workflow model can define input schema to constrain the variables passed when starting a workflow.

## Input Schema

Uses JSON Schema format to define the input schema:

```yml
id: my_model
name: test
inputs:
  type: object
  properties:
    a:
      type: integer
      default: 10
    user_name:
      type: string
  required:
    - a
```

## Passing Inputs When Starting

```rust
use acts::{Engine, Vars, Workflow};

let engine = Engine::new().start().unwrap();
let executor = engine.executor();

let mut vars = Vars::new();
vars.set("a", 100);
vars.set("user_name", "admin");
executor.proc().start("my_model", vars)?;
```

## Dynamic Input Setting

You can set the `inputs` value using `ModelBuilder`:

```rust
use acts::model::Workflow;

let mut workflow = Workflow::new("my_model", "my workflow")
    .set_inputs(serde_json::json!({
        "type": "object",
        "properties": {
            "a": { "type": "integer" },
            "b": { "type": "string" }
        }
    }));
```

## Step Inputs

Input data can also be received at the step level:

```yml
steps:
    - id: step1
      vars:
        - name: local_var
          value: '{{ inputs.a }}'
```
