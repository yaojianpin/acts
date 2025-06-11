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

# 0.10.6

- reset the build mode to bundled for store feature

# 0.11.0

- change store db to sqlite

# 0.12.0

- change the act yml format, use act: xx instead of !xx
- add setup to act and remove on_created, on_completed
- add act.expose for pack
- add nid for Message
- use 'do' act instead of 'cmd'
- expands executor with msg(), pack(), proc(), task(), act() and mode() instead of manager

# 0.12.1

- update act.set_output to act.expose
- keep act.expose only expose the vars to outputs
- fix the model tree output issue

# 0.12.3

- fix the test error with feature store

# 0.12.4

- fix test error for act each result check issue

# 0.12.5

- add export.msg unsub to support unsubscribe the messages by client
- fix the deadlock issue by subscribing with same client id by many times

# 0.13.0

- change the the query function to return PageData in trait DbSet for store collection
- add query_by and order_by to query fn
- add `mid` to message collection
- add ExecutorQuery to export list fn for msg, pack, proc, task, message

# 0.13.1

- fix: fix the memory store query error

# 0.13.2

- upgrate rquickjs to 0.8.1

# 0.13.3

- merge Luminvent's change
- fix: fix the clippy error

# 0.14.0

- improve the code quality
- add set_process_var for act_execution

# 0.15.0

- feat: allow to keep processes after completion
- fix: set process state if task is completed and is root task [#12](https://github.com/yaojianpin/acts/issues/12)

# 0.16.0

- feat: reafactoring the act package to support act extension [#8]
- feat: adding package meta struct to support package jsonschema definition [#9]
- feat: add acts-sqlite plugin
- feat: add acts-postgres plugin #[13]
- feat: add acts.cfg support
- feat: modify Config to support getting custom config section
- feat: add workflow.on events (manual, hook, chat)
- feat: add acts-state package to support get or set state
- feat: change the directory structure with acts, plugins, examples, benches, config

# 0.17.0
- feat: reactoring env module to add register_var trait
- feat: add "resects" user var to support get resects data from task context
- feat: add step env module to support get step vars by step id
- feat: change env var to $env
- feat: rename proc env_local to env
- feat: change event package params to Option<T>
- feat: skip initialization of plugin when there is no related section
- feat: add `acts.core.http` package plugin
- feat: add `pid` prefix to the acts.app.state package
- feat: support private vars (starts with `__`)
- feat: use `acts.toml` instead of `acts.cfg`
- feat: add `env.expose` in workflow to support set default outputs, the default outputs is `data`
- feat: add `acts.app.shell` package plugin to support nushell, bash and powershell
        support {{ }} refs the var in shell script
- feat: add `os` var in expression
- feat: use {{ }} for expression instead of ${ }
- feat: use var name directly in script or expression instead of $("var")
        use $get("var") instead of $("var")
        use $set("var", value) instead of $("var", value)
        use $inputs() instead of $act.inputs()
        use $data() instead of $act.data()

# 0.17.1
- fix: fix examples/plugins build issue when exclude examples

# 0.17.2
- fix: add info.MessageInfo uses property

# 0.18.0
- feat: change the workspace structure
- feat: use Query instead of ExecuteQuery
- feat: add `match` operation to FilterExpr
