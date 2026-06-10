# Examples

The following examples demonstrate various usage scenarios of the acts workflow engine.

## Basic Examples

| Example | Description | Path |
| ---- | ---- | ---- |
| Simple Loop | Using JavaScript to implement a loop accumulator | [examples/simple](https://github.com/yaojianpin/acts/tree/main/examples/simple) |
| Model Builder | Building workflows via Rust Builder API | [examples/model_build](https://github.com/yaojianpin/acts/tree/main/examples/model_build) |

## Interaction Examples

| Example | Description | Path |
| ---- | ---- | ---- |
| Action Interaction | Using IRQ interrupts to interact with clients | [examples/actions](https://github.com/yaojianpin/acts/tree/main/examples/actions) |
| Approval Process | Multi-role approval workflow (PM, GM) | [examples/approve](https://github.com/yaojianpin/acts/tree/main/examples/approve) |
| Message Notification | Using MSG to send one-way notifications | [examples/message](https://github.com/yaojianpin/acts/tree/main/examples/message) |

## Error & Timeout

| Example | Description | Path |
| ---- | ---- | ---- |
| Error Handling | Using catches to capture and handle errors | [examples/catches](https://github.com/yaojianpin/acts/tree/main/examples/catches) |
| Timeout Handling | Using timeouts to handle step timeouts | [examples/timeout](https://github.com/yaojianpin/acts/tree/main/examples/timeout) |

## Advanced Features

| Example | Description | Path |
| ---- | ---- | ---- |
| Event Driven | Using `on` events to trigger workflow start | [examples/event](https://github.com/yaojianpin/acts/tree/main/examples/event) |
| Subflow | Using subflow to call child workflows | [examples/subflow](https://github.com/yaojianpin/acts/tree/main/examples/subflow) |
| Custom Package | Creating and registering custom packages | [examples/package](https://github.com/yaojianpin/acts/tree/main/examples/package) |
| Custom Variables | Registering and using custom user variables | [examples/user_var](https://github.com/yaojianpin/acts/tree/main/examples/user_var) |

## Plugin Examples

| Example | Description | Path |
| ---- | ---- | ---- |
| HTTP Request | Sending HTTP requests via acts-package-http | [examples/plugins/http](https://github.com/yaojianpin/acts/tree/main/examples/plugins/http) |
| Shell Execution | Executing shell scripts via acts-package-shell | [examples/plugins/shell](https://github.com/yaojianpin/acts/tree/main/examples/plugins/shell) |
| State Management | Managing state via acts-package-state | [examples/plugins/state](https://github.com/yaojianpin/acts/tree/main/examples/plugins/state) |

## Running Examples

```bash
# Run approval process example
cargo run --example approve

# Run error handling example
cargo run --example catches

# Run timeout handling example
cargo run --example timeout
```
