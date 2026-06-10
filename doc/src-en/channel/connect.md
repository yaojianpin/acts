# Connect

Application services create and connect to the service via `Client`.

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());

// Connect to acts-server
client.connect().await?;
```

## Connection Options

```rust
use acts_channel::ChannelOptions;

let options = ChannelOptions {
    // Client identifier
    client_id: Some("my_client".to_string()),
    // Other config
    ..ChannelOptions::default()
};

let mut client = Client::new("http://localhost:8080", &options);
client.connect().await?;
```
