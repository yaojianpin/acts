# Sequence 顺序执行

使用 `acts.core.sequence` 对集合进行顺序链式执行，后一个序列的执行依赖上一个序列的完成。

```yml
name: test
steps:
    - id: step1
      vars:
        - name: items
          value:
            - u1
            - u2
      uses: acts.core.sequence
      params:
        in: '{{ items }}'
        acts:
          # 会生成 2 个 irq 活动，按顺序逐一执行
          - uses: acts.core.irq
            params:
              key: act1
```

## 与 parallel 的区别

| 类型 | 包 | 说明 |
| ---- | ---- | ---- |
| 并行 | `acts.core.parallel` | 所有子活动同时并行执行 |
| 顺序 | `acts.core.sequence` | 子活动按顺序逐个执行，后一个等待前一个完成 |
| 块 | `acts.core.block` | 按 `mode: sequence` 模式顺序执行嵌套 acts |

引擎会自动将 `index` 和 `value` 注入到每个子活动的变量上下文中。
