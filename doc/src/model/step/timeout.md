# 超时

当步骤需要检测超时时，可以通过 `timeouts` 定义超时处理。`timeouts` 是一个 `Step` 列表，每个 timeout 步骤通过 `if` 条件中的 `$cost_in()` 函数匹配时间阈值。

`$cost_in()` 支持的时间单位：
- `s` — 秒 (seconds)
- `m` — 分钟 (minutes)
- `h` — 小时 (hours)
- `d` — 天 (days)

```yml
name: test
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      timeouts:
        # 2秒后超时 - 发送消息
        - uses: acts.core.msg
          if: $cost_in('2s')
          params:
            key: step1_timeout_2s

        # 5秒后超时 - 发起中断请求
        - uses: acts.core.irq
          if: $cost_in('5s')
          params:
            key: step1_timeout_5s

        # 8秒后超时 - 触发错误
        - uses: acts.core.action
          if: $cost_in('8s')
          params:
            action: error
            options:
              ecode: err_timeout_8s
```

使用超时需要配置 tick 间隔：

```rust
let engine = EngineBuilder::new()
    .tick_interval_secs(1)
    .build()
    .start()
    .unwrap();
```
