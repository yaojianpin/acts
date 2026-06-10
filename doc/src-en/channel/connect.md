# Install acts-channel client

Install via `cargo`:

```bash
cargo add acts-channel
```

# Connect

Application services create and connect to the service via `ActsChannel`.

```rust
use acts_channel::ActsChannel;

let mut client = ActsChannel::connect("http://localhost:8080");
```

