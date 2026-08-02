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
use acts::Engine;
use acts_package_state::StatePackage;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .add_pacakge::<StatePackagePlugin>()
        .build()
        .start();
}
```