# Step

A Step is the basic execution unit of a workflow. Each step can use a built-in or custom package (`uses`) and pass parameters (`params`). Steps execute sequentially and can also include branches, error handling, and timeout handling.

```yml
name: test
steps:
    - id: step1
      name: step 1
      uses: acts.core.irq
      params:
        key: act1

    - id: step2
      name: step 2
      uses: acts.transform.set
      params:
        a: 10
```

## Step Attributes

| Key | Name | Description |
| ---- | ---- | ---- |
| id | ID | Unique node identifier |
| name | Name | Human-readable name, supports any characters |
| desc | Description | Step description |
| tag | Tag | Tag configuration |
| rn | Resource Name | Resource name for permission control |
| uses | Package | The package name, e.g. `acts.core.irq`, `acts.transform.set` |
| params | Parameters | Parameters passed to the package |
| vars | Variables | Local variable definitions |
| if | Condition | Skip execution based on condition, e.g. `${{ a }} > 0` |
| while | While condition | Loop: re-execute this step while the condition holds |
| catches | Catches | Error handling when step errors, type `Vec<Step>` |
| timeouts | Timeouts | Timeout handling, type `Vec<Step>` |
| branches | Branches | Step branches, a step can have multiple branches |
| next | Next | Jump to a specified step after the step completes |
| options | Options | Extra options, e.g. `exposes` to export variables |
| metadata | Metadata | Extra info for UI styling, not sent to client |

A `while` step is a bounded loop: the condition is re-evaluated before each
iteration and the step re-executes while it holds; once it fails the step is
skipped and the flow falls through to the next step declared after it:

```yml
steps:
    - id: add
      while: index < input
      uses: acts.transform.code
      params: |
          $set("value", value + index);
          $set("index", index + 1);

    - id: end
```

A step whose `if` condition fails is also skipped and falls through to the
next declared step (a self/backward `next` is then not taken), so `if` and
`next` keep their original meanings and `while` cannot be combined with `next`.

## Multi-Act Steps

When a step needs to execute multiple activities, use the `acts.core.block` package and nest child activity lists in `params`:

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
          - uses: acts.core.msg
            params:
              key: msg1
```

`acts.core.parallel` can execute over a collection in parallel, and `acts.core.sequence` can execute sequentially.
