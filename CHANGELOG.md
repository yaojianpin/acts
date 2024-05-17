# 0.10.0

- add `tokio_local!` to make env module working with `Context`
- add `quickjs` runtime in env module
- use `quickjs` runtime in `pack` instead of `wit`
- remove `start` function from `Engine`
- add `Builder` to build engine with different config
- add workflow `env` to support all workflows can get env vars and set locally
- simplified the options of the `error` action
- merge action state to task state
- add engine channel to receive messages by options and the channel messages can re-send if not acked

# 0.10.1
- remove the warning code
- fix the doc test error
- rename engine.emitter to engine.channel
- rename data::message emit_id to chan_id, emit_pattern to chan_pattern
- delete data::message emit_count
- remove default feature
