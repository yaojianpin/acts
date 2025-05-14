# acts-postgres

The acts postgres plugin for acts store. 

## Installation

create `acts.cfg` in current dir
```
postgres {
    database_url: "postgresql://<your connection string>"
}
```

```bash
cargo add acts-postgres
```

## Example

```rust,no_run
use acts::EngineBuilder;
use acts_postgres::PostgresStore;

#[tokio::main]
async fn main() {
    let engine = EngineBuilder::new()
        .add_plugin(&PostgresStore)
        .build()
        .start();
}
```