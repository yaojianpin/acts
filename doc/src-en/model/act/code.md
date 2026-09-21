# Code

Use `acts.transform.code.javascript` to execute JavaScript code (QuickJS engine) for variable computation, data transformation, and conditional logic within a workflow.

Task data reaches the script through `${{ }}` placeholders (evaluated as CEL expressions by the engine), and the script hands data back by returning a JSON object.

```yml
steps:
    - id: step1
      uses: acts.transform.code.javascript
      params: |
        return { sum: ${{ a }} + ${{ b }}, message: "Result: " + (${{ a }} + ${{ b }}) };
```

## Data exchange

| Mechanism | Description |
| ---- | ---- |
| `${{ expr }}` | Injects the CEL expression's result into the script (variables, `$env`, `$data()`, `$inputs()`, …) |
| `return { ... }` | Returns a JSON object written into the task data as the step's output |

## Use Cases

**Variable computation:**
```yml
- uses: acts.transform.code.javascript
  params: |
    return { count: ${{ count }} + 1 };
```

**Array operations:**
```yml
- uses: acts.transform.code.javascript
  params: |
    let a = ["u1", "u2"];
    let b = ["u2", "u3"];
    return { merged: a.concat(b) };
```

**Conditional checks and errors:**
```yml
- uses: acts.transform.code.javascript
  params: |
    if (${{ status }} != "ok") {
      return { ecode: "invalid_status" };
    }
    return {};
```
