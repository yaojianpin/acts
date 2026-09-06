
## 导出变量

使用 `exposes` 控制哪些变量在完成时被导出：

```yml
id: m1
name: test
exposes:
  - name: a
  - name: result
steps:
    - id: step1
      uses: acts.transform.code
      params: |
        let a = $get("a");
        $set("result", a * 2);
```

`exposes` 项省略 `type` 时不会默认当作 `string`：导出时按变量在运行时的实际类型
校验并导出。提供字面量 `value` 时按其值推断类型（如 `value: 10` 即 `number`）；
显式声明 `type` 时仍按声明类型严格校验。

## 步骤导出

步骤也可以通过 `options.exposes` 导出变量到父级（工作流或上层步骤）：

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      exposes:
        - name: v
```
