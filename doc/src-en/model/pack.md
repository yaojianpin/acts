# Package

Packages are reusable functional modules used via `uses` in steps and activities.

## Built-in Packages

### Core Packages

| Package | Type | Description |
| ---- | ---- | ---- |
| `acts.core.irq` | IRQ | Interrupt request, pauses for client response |
| `acts.core.msg` | MSG | One-way message to client |
| `acts.core.block` | IRQ | Block with nested acts (supports sequence mode) |
| `acts.core.parallel` | IRQ | Parallel execution over a list |
| `acts.core.sequence` | IRQ | Sequential execution over a list |
| `acts.core.subflow` | IRQ | Invoke sub-workflow |
| `acts.core.action` | MSG | Engine action (e.g. trigger error) |

### Transform Packages

| Package | Type | Description |
| ---- | ---- | ---- |
| `acts.transform.set` | MSG | Set variable values |
| `acts.transform.code` | IRQ | Execute JavaScript code (QuickJS engine) |

### Event Packages

| Package | Type | Description |
| ---- | ---- | ---- |
| `acts.event.manual` | Event | Synchronous manual event |
| `acts.event.hook` | Event | Hook event (waits for completion) |
| `acts.event.chat` | Event | Chat event |

## Usage Example

```yml
steps:
    # IRQ — interrupt and wait for client response
    - id: step1
      uses: acts.core.irq
      params:
        key: act1

    # MSG — one-way notification
    - id: step2
      uses: acts.core.msg
      params:
        key: notification

    # Set variable
    - id: step3
      uses: acts.transform.set
      params:
        a: 10

    # Execute JavaScript
    - id: step4
      uses: acts.transform.code
      params: |
        return { result: a + 10 };
```

## Custom Package

You can create custom packages. Refer to the [package example](https://github.com/yaojianpin/acts/tree/main/examples/package) for details.
