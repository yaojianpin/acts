# Code 代码执行

使用 `acts.app.javascript` 执行 JavaScript 代码（QuickJS 引擎），可以在流程中进行变量计算、数据转换、条件判断等。

任务数据通过 `${{ }}` 占位符（由引擎的表达式求值器求值）注入脚本，脚本通过返回一个 JSON 对象把数据交回流程。

```yml
steps:
    - id: step1
      uses: acts.app.javascript
      params: |
        return { sum: ${{ a }} + ${{ b }}, message: "计算结果: " + (${{ a }} + ${{ b }}) };
```

## 数据交换

| 方式 | 说明 |
| ---- | ---- |
| `${{ expr }}` | 把表达式的结果注入脚本（变量、`$env`、`$data()`、`$inputs()` 等） |
| `return { ... }` | 返回一个 JSON 对象，作为该步骤的输出写入任务数据 |

## 使用场景

**变量计算：**
```yml
- uses: acts.app.javascript
  params: |
    return { count: ${{ count }} + 1 };
```

**数组操作：**
```yml
- uses: acts.app.javascript
  params: |
    let a = ["u1", "u2"];
    let b = ["u2", "u3"];
    return { merged: a.concat(b) };
```

**条件判断与错误：**
```yml
- uses: acts.app.javascript
  params: |
    if (${{ status }} != "ok") {
      return { ecode: "invalid_status" };
    }
    return {};
```
