# Block 块

使用 `acts.core.block` 将多个活动组合成一个块执行。支持 `sequence`（顺序）和 `parallel`（并行）两种模式。

```yml
name: test
steps:
    - id: step1
      uses: acts.core.block
      params:
        # 执行模式: sequence (顺序) 或 parallel (并行)
        mode: sequence
        acts:
          - uses: acts.transform.set
            params:
              count: 0
          - uses: acts.core.irq
            params:
              key: act1
          - uses: acts.core.msg
            params:
              key: done
```

## 模式对比

| 模式 | 说明 |
| ---- | ---- |
| `sequence` | 按顺序逐个执行子活动，后一个等待前一个完成 |
| `parallel` | 所有子活动同时并行执行 |

## 变量导出

块内活动可以通过 `options.exposes` 将变量导出到父节点：

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
            options:
              exposes:
                - name: result
```
