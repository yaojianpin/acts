# acts-sqlite

The acts sqlite store plugin for acts. 

## Installation


create `config/acts.cfg` in current dir
```no_compile
sqlite {
    database_url: "sqlite://<your file path>"
}
```

```no_compile
cargo add acts-store-sqlite
```

## Example

```rust,no_run
use acts::{EngineBuilder,Result};
use acts_store_sqlite::SqliteStore;

#[tokio::main]
async fn main() -> Result<()> {
    let engine = EngineBuilder::new()
        .add_plugin(&SqliteStore)
        .build()
        .await?
        .start();
    
    Ok(())
}
```