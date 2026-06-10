# Code 代码执行

使用 `acts.transform.code` 执行 JavaScript 代码（QuickJS 引擎），可以在流程中进行变量计算、数据转换、条件判断等。

```yml
steps:
    - id: step1
      uses: acts.transform.code
      params: |
        let x = $get("a");
        let y = $get("b");
        $set("sum", x + y);
        $set("message", "计算结果: " + (x + y));
```

## 内置函数

| 函数 | 说明 |
| ---- | ---- |
| `$get("key")` | 获取变量值 |
| `$set("key", value)` | 设置变量值 |
| `$ecode()` | 获取当前错误码 |
| `$cost_in('2s')` | 判断时间是否超过指定值 |
| `$inputs()` | 获取上一步的输入数据 |
| `$data()` | 获取当前数据 |
| `$env("key")` | 获取环境变量 |

## 使用场景

**变量计算：**
```yml
- uses: acts.transform.code
  params: |
    let count = $get("count") || 0;
    $set("count", count + 1);
```

**数组操作：**
```yml
- uses: acts.transform.code
  params: |
    let a = ["u1", "u2"];
    let b = ["u2", "u3"];
    $set("merged", a.concat(b));
```

**条件判断与错误：**
```yml
- uses: acts.transform.code
  params: |
    if ($get("status") != "ok") {
      $set("ecode", "invalid_status");
    }
```
