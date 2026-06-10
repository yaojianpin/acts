# 输出

工作流的 `outputs` 定义了流程输出的 JSON Schema。当流程正常结束时，流程完成消息会包含 `outputs` 数据。

```yml
id: m1
name: test
vars:
    - name: a
      value: 5
outputs:
  type: object
  properties:
    a:
      type: integer
```

## 导出变量

使用 `options.exposes` 控制哪些变量在完成时被导出：

```yml
id: m1
name: test
options:
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

## 步骤导出

步骤也可以通过 `options.exposes` 导出变量到父级（工作流或上层步骤）：

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      options:
        exposes:
          - name: v
```
