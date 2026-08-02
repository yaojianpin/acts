# Subflow 子流程

使用 `acts.core.subflow` 调用另一个工作流模型（子流程）。

```yml
name: test
steps:
    - id: step1
      uses: acts.core.subflow
      params:
        # 子流程的模型 ID
        to: sub_workflow_id
        # 传递给子流程的输入数据
        a: '${{ value }}'
```

## 子流程定义

子流程是一个独立的工作流模型：

```yml
id: sub_workflow_id
name: sub_flow
inputs:
  type: object
  properties:
    a:
      type: integer
outputs:
  type: object
  properties:
    result:
      type: string
steps:
    - id: sub_step1
      uses: acts.core.irq
      params:
        key: sub_act

    - id: sub_step2
      uses: acts.core.msg
      params:
        key: sub_done
```

## 数据传递

子流程的输入通过 `params` 传递，子流程的输出通过 `options.exposes` 导出回父流程：

```yml
steps:
    - id: step1
      uses: acts.core.subflow
      params:
        to: sub_workflow_id
        input_value: '${{ parent_var }}'
      options:
        exposes:
          - name: result
```
