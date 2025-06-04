# acts-package-http

The acts state package plugin for acts. 

## Installation


```bash
cargo add acts-package-http
```

## Example

```rust,no_run
use acts::EngineBuilder;
use acts_package_http::HttpPackagePlugin;

#[tokio::main]
async fn main() {
    let engine = EngineBuilder::new()
        .add_plugin(&HttpPackagePlugin)
        .build()
        .start();
}
```