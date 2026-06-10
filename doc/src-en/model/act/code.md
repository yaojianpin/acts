# Code

Use `acts.transform.code` to execute JavaScript code (QuickJS engine) for variable computation, data transformation, and conditional logic within a workflow.

```yml
steps:
    - id: step1
      uses: acts.transform.code
      params: |
        let x = $get("a");
        let y = $get("b");
        $set("sum", x + y);
        $set("message", "Result: " + (x + y));
```

## Built-in Functions

| Function | Description |
| ---- | ---- |
| `$get("key")` | Get variable value |
| `$set("key", value)` | Set variable value |
| `$ecode()` | Get current error code |
| `$cost_in('2s')` | Check if time exceeds the specified duration |
| `$inputs()` | Get previous step's input data |
| `$data()` | Get current data |
| `$env("key")` | Get environment variable |

## Use Cases

**Variable computation:**
```yml
- uses: acts.transform.code
  params: |
    let count = $get("count") || 0;
    $set("count", count + 1);
```

**Array operations:**
```yml
- uses: acts.transform.code
  params: |
    let a = ["u1", "u2"];
    let b = ["u2", "u3"];
    $set("merged", a.concat(b));
```

**Conditional checks and errors:**
```yml
- uses: acts.transform.code
  params: |
    if ($get("status") != "ok") {
      $set("ecode", "invalid_status");
    }
```
