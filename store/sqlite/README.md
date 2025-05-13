# acts-sqlite

The acts sqlite plugin for acts store. 

## Installation


```bash
cargo add acts-sqlite
```

## Example

```rust,no_run
use acts::EngineBuilder;
use acts_sqlite::SqliteStore;

#[tokio::main]
async fn main() {
    let engine = EngineBuilder::new()
        .add_plugin(&SqliteStore)
        .build()
        .start();
}
```