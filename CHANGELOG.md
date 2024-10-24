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

# 0.10.2

- update readme.md
- add homepage

# 0.10.3

- remove action result, the time will caculate by acts-channel
- refactor the info struct to make is easier to understand.

# 0.10.4

- modify the test error with 'store' feature

# 0.10.5

- remove the warnings in rust 1.82
- remove the duckdb bundle feature
