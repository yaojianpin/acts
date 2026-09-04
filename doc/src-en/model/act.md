# Act

An Act is the actual action execution body, using `uses` to specify a functional package.

## Act Attributes

| Key | Name | Description |
| ---- | ---- | ---- |
| id | ID | Activity identifier |
| name | Name | Activity name |
| uses | Package | Package name |
| params | Parameters | Package parameters |
| inputs | Inputs | Input data |
| outputs | Outputs | Output data |
| options | Options | Extra options (e.g. exposes) |

## Usage

### Single Act (step-level)

```yml
steps:
    - id: step1
      uses: acts.core.irq
      params:
        key: act1
```

### Multiple Acts (nested in block)

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
          - uses: acts.core.msg
            params:
              key: msg1
```

## Built-in Packages

| Package | Type | Description |
| ---- | ---- | ---- |
| `acts.core.irq` | IRQ | Interrupt request — pauses for client response |
| `acts.core.msg` | MSG | One-way message to client |
| `acts.core.block` | IRQ | Block with nested acts |
| `acts.core.parallel` | IRQ | Parallel execution over a list |
| `acts.core.sequence` | IRQ | Sequential execution over a list |
| `acts.core.subflow` | IRQ | Invoke sub-workflow |
| `acts.core.action` | MSG | Engine action |
| `acts.transform.set` | MSG | Set variable values |
| `acts.transform.code` | IRQ | Execute JavaScript |
