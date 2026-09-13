# 访问控制

引擎配置里的 `[acl]` 段是可选的。没有该段时不启用任何校验——所有请求都放行，
即开启 ACL 之前的行为。

该段"存在即启用"。也可以用 `enabled = false` 显式关闭，但没有写这个段的理由。

## Token 与角色

请求携带 token，token 选中一个**角色**，角色的 `allow` / `deny`
**动作模式**决定是否放行。`deny` 优先；由于该段本身就是开关，没有 token
（或 token 未命中任何角色）的请求默认被拒绝，除非 `default_role` 指定了兜底角色。

Token 按 **SHA-256 摘要**比较：写 `sha256:<64 位十六进制>` 可让明文不落配置；
也可以直接写 token 本身，由服务端在加载时哈希。

```toml
[acl]
# 无 token / token 未命中角色时的兜底角色；不写则拒绝这类请求
default_role = "guest"

# 简写：一把不受限的 token（等价于 requirepass）
token = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"

[[acl.role]]
name = "operator"
tokens = ["sha256:2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"]
allow = ["model:ls", "model:get", "proc:ls", "proc:get", "task:*", "msg:ls",
         "msg:ack", "snap:get", "snap:ls", "acl:whoami"]
deny = ["model:rm", "pack:publish"]
snapshot = { secrets = ["$subject"], profile = ["$subject/*"] }

[[acl.role]]
name = "guest"
tokens = ["sha256:..."]
allow = ["model:ls", "acl:whoami"]
```

配置错误一律是启动错误，绝不静默放行或静默拒绝：模式编译失败、同一 token
被分配给两个角色、`default_role` 指向不存在的角色、或已启用但没有任何角色声明 token。

## 动作名

`allow`/`deny` 匹配共享动作表里的动作名，支持 `*`、`?` 通配——例如 `act:*`
覆盖全部 act 操作。这些名字与 CLI、channel 客户端使用的完全一致。

| 分类 | 动作 |
| --- | --- |
| 读取 | `model:ls` `model:get` `proc:ls` `proc:get` `task:ls` `task:get` `msg:ls` `msg:get` `evt:ls` `evt:get` `pack:ls` `pack:get` `snap:get` `snap:ls` |
| 写入 | `model:deploy` `pack:publish` `snap:upsert` `snap:remove` |
| 控制 | `proc:start` `proc:start_from_model` `act:push` `act:remove` `act:submit` `act:complete` `act:abort` `act:cancel` `act:back` `act:skip` `act:error` `evt:start` `msg:ack` |
| 管理 | `model:rm` `pack:rm` `msg:rm` `msg:clear` `msg:redo` `msg:unsub` |

`allow = ["*"]` 表示不受限：所有动作放行，所有 snapshot scope 也放行。
`acl:whoami` 返回调用者自身的身份与生效模式；对已认证调用者隐式放行，
因此可以作为启动自检使用，而不会额外放开任何权限。

## Snapshot 的 scope 归属

Snapshot target 由 `target` × `scope` 寻址（见[快照密封数据](./model/act.md)）。
角色的 `snapshot` 表限定该角色拥有哪些 target 的哪些 scope；`$subject`
会替换为角色名，所以 `["$subject"]` 表示"只读我自己的 scope"。

该规则在两处强制：

- `snap:*` 动作上——读写超出调用者集合的 scope 会被拒绝，`snap:ls`
  只返回该主体拥有的 scope，一个租户无法枚举他人的数据。
- **密封时**——通过动作启动的流程会带上调用者的规则，调度器在把 snapshot
  值冻结进任务前会再校验一次。因此即使有人用别人的 `uid` 启动流程，
  工作流也读不到另一个主体已密封的数据。

进程内直接调用 executor 启动的流程，或由触发器启动的流程不带调用者身份，
因此不受限制。

## 各传输的凭证

| 传输 | token 的携带方式 |
| --- | --- |
| gRPC | 每次请求的 `authorization: Bearer <token>` metadata，包括 `on_message` 订阅 |
| HTTP | `authorization: Bearer <token>` 请求头；`/health` 保持开放以便探活 |
| NATS | 动作 JSON body 里的 `token` 字段——broker 认证的是连接，不是单次请求 |

## 客户端

```bash
# CLI：命令行参数优先于环境变量
acts-cli --token "$TOKEN"
ACTS_TOKEN="$TOKEN" acts-cli
```

CLI 在进入 REPL 前先用 `acl:whoami` 确认身份，因此 token 缺失或过期会在启动时
暴露，而不是拖到第一条命令。

```rust,no_run
use acts_channel::ActsChannel;

let mut client = ActsChannel::connect_with_token("http://127.0.0.1:10080", Some(token)).await?;
```

`connect` 是不带 token 的形式，只能用于没有 `[acl]` 段的服务端。
