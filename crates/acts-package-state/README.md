# acts-sqlite

The acts state package plugin for acts. 

## Installation

create `config/acts.cfg` in current dir
```
state {
    uri: "redis://<your connection path>"
}
```

```bash
cargo add acts-package-state
```

## Example

```rust,no_run
use acts::EngineBuilder;
use acts_package_state::StatePackagePlugin;

#[tokio::main]
async fn main() {
    let engine = EngineBuilder::new()
        .add_plugin(&StatePackagePlugin)
        .build()
        .start();
}
```