# Parallel 并行执行

使用 `acts.core.parallel` 对集合进行并行执行，所有子活动同时发起，互不依赖。

```yml
name: test
steps:
    - id: step1
      vars:
        - name: items
          value:
            - u1
            - u2
            - u3
      uses: acts.core.parallel
      params:
        in: '{{ items }}'
        acts:
          # 会生成 3 个 irq 活动，同时并行执行
          - uses: acts.core.irq
            params:
              key: act1
```

## 与 sequence、block 的区别

| 类型 | 包 | 说明 |
| ---- | ---- | ---- |
| 并行 | `acts.core.parallel` | 所有子活动同时并行执行 |
| 顺序 | `acts.core.sequence` | 子活动按顺序逐个执行，后一个等待前一个完成 |
| 块 | `acts.core.block` | 按 `mode: sequence` 或 `mode: parallel` 模式执行嵌套 acts |

## 变量注入

引擎会自动将 `index` 和 `value` 注入到每个子活动的变量上下文中，可以在子活动中通过 `{{ index }}` 和 `{{ value }}` 访问。

## 代码生成集合

可以结合 `acts.transform.code` 动态生成集合：

```yml
steps:
    - id: step1
      uses: acts.transform.code
      params: |
        let list = ["u1", "u2", "u3"];
        $set("items", list);
    - id: step2
      uses: acts.core.parallel
      params:
        in: '{{ items }}'
        acts:
          - uses: acts.core.irq
            params:
              key: act2
```
