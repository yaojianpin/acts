# acts-postgres

The acts postgres plugin for acts. 

## Installation

create `config/acts.cfg` in current dir
```no_compile
postgres {
    database_url: "postgresql://<your connection string>"
}
```

```bash,no_compile
cargo add acts-store-postgres
```

## Example

```rust,no_run
use acts::{EngineBuilder, Result};
use acts_store_postgres::PostgresStore;

#[tokio::main]
async fn main() -> Result<()> {
    let engine = EngineBuilder::new()
        .add_plugin(&PostgresStore)
        .build().await?
        .start();

    Ok(())
}
```