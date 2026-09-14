# 访问控制

引擎配置里的 `[acl]` 段决定这台引擎是否认得调用者。**没有该段时，引擎对匿名开放
但只读**：所有请求都归属到内置的 `anonymous` 主体，它可以 list/get 模型、流程、
任务、消息、事件与包，仅此而已——没有写入、没有控制类动作、没有管理类动作、
没有 snapshot scope、也不能订阅。未配置的部署是用来"看"引擎的，不是用来改它、
也不是用来读属于别人的数据的。

两种离开该默认的姿态：

- 加上 `[acl]`——最小可用的一段就是一个 `token`，它让该 token 拥有一切
  （等价于 requirepass），并让所有调用者从匿名变为已认证；
- 在 `[acl]` 里写 `enabled = false`（显式退出）：不做任何校验，所有调用者不受限。
  这既是开启 ACL 之前的行为，如今也是**刻意的选择**，而不再是"没写配置"的结果。

## Token 与角色

请求携带 token，token 选中一个**角色**，角色的 `allow` / `deny`
**动作模式**决定是否放行。`deny` 优先。没有 token（或 token 未命中任何角色）的请求
默认被拒绝，除非 `default_role` 指定了兜底角色——想保留只读默认、同时配置其它规则时，
就用 `[[acl.role]] name = "anonymous"` 加上 `default_role = "anonymous"`。

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
workdir = "/srv/acts"

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
| 订阅 | `msg:sub` |
| 管理 | `model:rm` `pack:rm` `msg:rm` `msg:clear` `msg:redo` `msg:unsub` |

`allow = ["*"]` 表示不受限：所有动作放行，所有 snapshot scope 也放行。
`acl:whoami` 返回调用者自身的身份与生效模式；对已认证调用者隐式放行，
因此可以作为启动自检使用，而不会额外放开任何权限。

没有 `[acl]` 段时归属的 `anonymous` 主体，拿到的正是上表"读取"一组去掉 snapshot
两项——所有针对模型、流程、任务、消息、事件、包的 `*:ls` / `*:get`。这是引擎无法
指名道姓的调用者可以被授予的最小集合：`msg:sub` 不在其中，因为一条订阅流既承载实时
载荷、又会为它持有的通道逐条写入投递行；`snap:get`/`snap:ls` 也不在其中，因为
snapshot scope 只有在策略点名时才有归属。

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

## 消息面

消息与投递同其它操作一样，由**动作授权**决定——发出它的流程归谁所有，并不影响
谁能读它、谁能 ack 它。

- **订阅是一个动作。** 打开一条流（gRPC `on_message`、SSE `/msg/sse`）需要角色
  的 `allow` 里含 `msg:sub`；不含时传输层直接回 `PERMISSION_DENIED` / `403`，
  不发流。传输层把收到的 client id 交给该动作，并用动作返回的键注册通道，因此
  "被校验的路径"与"实际占用的键"不可能各走各的。
- **键按主体划分命名空间**：`{subject}/{传输自身的 id}`——主体作前缀，后面接传输
  自己的 client id（SSE 保留自己的 `acts-flow-client-` 段）。因此第二个调用者用
  别的主体已经用过的 client id 订阅时，得到的是**另一个**通道，而不是顶替那个主体
  的 handler。`msg:unsub` 由同一个 id 组合出同一个键，所以调用者只能取消自己命名
  空间里的通道；角色名含 `/` 会在加载配置时被拒绝，前缀的无歧义正是靠这一点保证。
- **投递只看过滤器与授权**：通道会收到所有匹配它自报的 `type`/`state`/`uses`/
  `options` glob 的消息，无论发出它的流程是谁启动的；拿到消息后能做什么，由调用者
  持有的动作决定。不希望读到消息载荷的角色，就是没有 `msg:sub`（以及没有
  `msg:ls`/`msg:get`）的角色。
- `msg:ack` 与 `msg:unsub` 同样是普通动作：拿到授权就能 ack 任意投递 id、取消自己
  命名空间内的任意通道。投递 id 不是按客户端寻址的，因此 `msg:ack` 应按"写入"级别
  授予——它的持有者可以压掉别的调用者尚未 ack 的投递。

## 各层各管什么

三类规则，各自在能表达它的那一层校验：

| 规则 | 作用对象 | 依据 |
| --- | --- | --- |
| 动作权限 | 每一个操作，同一张表 | 角色的 `allow`/`deny` 模式 |
| snapshot scope 归属 | `snap:*` 动作，以及密封时再校验一次 | 角色的 `snapshot` 表（`$subject`） |
| 通道命名空间 | 通道键与 `msg:unsub` | 已认证主体 |

有两个入口**不**经过动作表，也不受其约束：进程内直接调用 `Engine::executor()`
启动的流程、以及由触发器启动的流程不带调用者身份，不受限制；引擎自身的内部操作
也不是请求。未配置引擎的匿名调用者恰恰是反例中的正例——它**是**一个调用者，
所以照样走那张表，拿到上面所述的只读子集。


## 目录控制

配置里写的是**根目录**：`workdir` 可写在 `[acl]` 上，也可写在单个角色上覆盖。策略启动的
每个流程得到的是它下面的自己的目录 `<workdir>/<pid>`，文件系统访问被限定在那里；进程 id
成为路径的一段——因此不能作为单个安全目录名的 pid（空、`.`、`..`，或含路径分隔符、冒号）
会被拒绝，而不是被放到根目录之外。

根目录走与 scope 权威同一条私有通道——它本身就是权威的一部分：随 owner 权威封入进程
env（`$env` 代理拒绝该私有键），随进程持久化，且不作为启动参数下发，因此调用者无法指定
流程被限定到哪个目录。**流程自己的目录**（即 `<root>/<pid>`）由三处读到：act 用
`Context::workdir()`，工作流脚本用 `$env.WORK_DIR`，两者指的是同一个目录，且都不是配置
里写的那个根。`$env.WORK_DIR` 由引擎应答，写入会被丢弃，脚本无法改写自己所在的目录。

目录的生命周期与进程的持久行一致：进程结束且消息投递结清后，sweeper 删除行时一并
删除目录（投递出错、等待人工重投的流程保留行，目录也随之保留）；从未落盘的启动则
立即删掉自己的目录。因此留在目录里的文件不会比流程本身活得更久。

`acts.app.shell` 使用它：脚本以该目录为工作目录运行，`HOME`、`TMPDIR`/`TEMP`/`TMP`、
`PWD` 都指向其中，`ACTS_WORKDIR` 让脚本能直接引用自己的目录。脚本中出现绝对路径
（`/etc/passwd`、`C:\Windows`）或 `..` 段时，会在运行前被拒绝。

这项文本检查是**策略，不是沙箱**：它的价值在于让直接越界变成显式失败而不是静默成功，
但 shell 能以文本检查无法跟进的方式拼出路径（`a=/etc; cat $a/passwd`、工作目录内的
符号链接）——真正生效的包含是子进程的工作目录。面对恶意工作流应依赖操作系统边界
（容器/命名空间），按进程分目录在此期间用于避免互相踩踏。

不配置 `workdir` 时不做任何目录控制，进程可以访问服务端账号能访问的一切，
即该选项出现之前的行为。

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

`connect` 是不带 token 的形式，即匿名调用者：对没有 `[acl]` 段的服务端只能读，
对已配置的服务端则会被拒绝，除非 `default_role` 接纳它。
