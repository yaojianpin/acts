# 活动列表

一个步骤可以直接指定 `uses` 和 `params` 来执行单个活动。当需要执行多个活动时，使用 `acts.core.block` 包在 `params` 中嵌套定义子活动列表。

## 单个活动

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
```

## 多个活动（顺序执行）

```yml
steps:
    - id: step1
      uses: acts.core.block
      params:
        mode: sequence
        acts:
          - uses: acts.transform.set
            params:
              a: 10
              list:
                - u1
                - u2

          - uses: acts.core.irq
            params:
              key: act1

          - uses: acts.core.msg
            params:
              key: msg1
```

## 并行执行

```yml
steps:
    - id: step1
      uses: acts.core.parallel
      params:
        in: '{{ list }}'
        acts:
          - uses: acts.core.irq
            params:
              key: act2
```

## 顺序执行

```yml
steps:
    - id: step1
      uses: acts.core.sequence
      params:
        in: '{{ list }}'
        acts:
          - uses: acts.core.irq
            params:
              key: act2
```

更多活动内容请参见 [`活动`](../act.md)。
