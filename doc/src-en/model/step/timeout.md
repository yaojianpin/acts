# Step Timeout

When a step exceeds a specified time, use `timeouts` to define timeout handling logic.

## Basic Usage

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
      timeouts:
        # Trigger message in >=2 seconds and < 8 seconds
        - uses: acts.core.msg
          if: $cost_in('2s', '8s')
          params:
            key: step1_timeout_2s

        # Trigger error after 8 seconds
        - uses: acts.core.action
          if: $cost_in('8s')
          params:
            action: error
            options:
              ecode: err_timeout_8s
```

## Time Duration

`$cost_in()` supports the following duration formats:

| Format | Example | Description |
| ---- | ---- | ---- |
| Seconds | `$cost_in('2s')` | >= 2 seconds |
| Minutes | `$cost_in('5m')` | >= 5 minutes |
| Hours | `$cost_in('2h')` | >= 2 hours |
| Days | `$cost_in('1d')` | >= 1 day |
| Range | `$cost_in('1d', '2d')` | >= 1 day and < 2 days |

## Timeout Check Interval

Set the tick interval for timeout checking via `options`:

```yml
options:
  tick_interval: 500
```

The default value is 1000 (milliseconds). The example sets it to 500ms for more frequent checks.
