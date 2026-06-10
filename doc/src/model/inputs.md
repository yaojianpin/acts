# 输入

工作流的 `inputs` 定义了流程模型的输入 JSON Schema，用于在启动工作流时验证输入数据。

```yml
id: m1
name: test
inputs:
  type: object
  properties:
    a:
      type: integer
      default: 5
    b:
      type: string
      default: abc
```

## 启动时动态修改

工作流的 `inputs` 可以在启动时通过 `vars` 参数进行动态赋值：

```rust
let mut vars = Vars::new();
vars.set("a", 100);
vars.set("b", "new_value");
executor.proc().start(&workflow.id, vars)?;
```

## 步骤输入

步骤也可以定义 `vars` 作为本地变量：

```yml
steps:
    - id: step1
      vars:
        - name: local_var
          value: 10
      uses: acts.core.irq
      params:
        key: act1
```
