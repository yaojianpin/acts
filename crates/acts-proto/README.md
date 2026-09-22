# acts-proto

The generated bindings of the acts gRPC protocol — the `ActsService`
definition and the messages that cross the wire — shared by both ends:

| who                  | half it uses                                    |
|----------------------|-------------------------------------------------|
| `acts-channel`       | `acts_service_client::ActsServiceClient`, `Message`, `MessageOptions` |
| `acts-plugin-grpc`   | `acts_service_server::ActsService`, `ActsServiceServer` |

Nothing else belongs here: no client ergonomics, no engine. A change to the
protocol is a change to `proto/acts.proto`, regenerated with

```bash
cargo build -p acts-proto --features codegen
```

which writes the committed `proto/acts.grpc.rs` (it needs no system `protoc`:
the build script points `PROTOC` at `protoc-bin-vendored`).

```protobuf
service ActsService {
  rpc Send(Message) returns (Message) {}
  rpc OnMessage(MessageOptions) returns (stream Message) {}
}
```
