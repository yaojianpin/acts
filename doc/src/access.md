# 访问控制

引擎的每一个操作，都要拿执行它的那个调用者身份做一次校验。引擎配置里的 `[acl]` 段
就是给这些调用者命名的。**没有该段时，引擎对任何人开放，但只交出目录**：所有请求都
归属到内置的 `anonymous` 主体，它能 list/get **模型与包**，仅此而已——没有别的读取、
没有写入、没有控制类动作、没有管理类动作、没有 snapshot scope、也不能订阅。未配置的
部署是用来"看"已经部署了什么的，不是用来改它、也不是用来读某个流程、某条消息或某个
触发器里的内容。

两种离开该默认的姿态：

- 加上 `[acl]`——最小可用的一段就是一个 `token`，它让该 token 拥有一切
  （等价于 requirepass），并让所有调用者从匿名变为已认证；
- 在 `[acl]` 里写 `enabled = false`（显式退出）：不做任何校验，所有调用者不受限。
  这既是开启 ACL 之前的行为，如今也是**刻意的选择**，而不再是"没写配置"的结果。
  嵌入式调用者用 `Engine::builder().disable_acl()` 表达同一件事——测试和本地演示
  用的就是这个设置。

任何地方都没有**隐式**的不受限策略：没写 `[acl]` 不是，没人认领的流程不是，
没人点名的 snapshot scope 也不是。它们各自对应下面写明的"什么都不读"的情形。

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
| 嵌入式 | `ext:register_var`（`ext().register_package` 走 `pack:publish`） |

最后一行是嵌入式调用者自己的接口——把用户变量模块装进表达式环境、发布包定义。
线上动作表里没有任何一项通到它，因此传输层的调用者够不着；扩展自己所托管的引擎的
嵌入式调用者，传的是它自己的身份（见[executor](#executor)）。

`allow = ["*"]` 表示不受限：所有动作放行，所有 snapshot scope 也放行。
`acl:whoami` 返回调用者自身的身份与生效模式；对已认证调用者隐式放行，
因此可以作为启动自检使用，而不会额外放开任何权限。

没有 `[acl]` 段时归属的 `anonymous` 主体，拿到的正是
`model:ls` `model:get` `pack:ls` `pack:get`——目录。这是引擎无法指名道姓的调用者
可以被授予的最小集合，而且这份清单由测试钉死，不留给解读。其余全部不在其中，
包括命名调用者看来理所当然的读取：一条流程行指名了谁跑了什么，一条投递指名了它
本是发给谁的，一个触发器指名了它将启动哪个模型——引擎认不出的调用者，不是这些行
所描述的那个人。`msg:sub` 同理，还多一条理由：一条订阅流既承载实时载荷，又会为它
持有的通道逐条写入投递行；`snap:get`/`snap:ls` 也不在其中，因为 snapshot scope
只有在策略点名时才有归属。

## executor

引擎的操作收在同一个对象上，即 executor，它**每个方法都在运行前先做一次校验**——
`model().deploy()`、`proc().start()` 以及其余全部。创建时就绑定了调用者：

```rust
// 传输层的请求：token 解析完之后
let executor = engine.executor(&principal);
executor.proc().start("my_model", vars).await?;

// 没有携带 token 的请求
let executor = engine.executor(&engine.anonymous());

// 引擎自身的操作；测试与本地演示也传这个
let executor = engine.executor(&Principal::unrestricted());
```

executor 还决定它启动的流程**带上什么**：`proc().start()` 与 `evt().start()`
把该主体的 snapshot scope 与 workdir 根封进流程，因此调用者无法通过在请求里塞一个
权威来放宽自己的读取范围。同一个模型的两个调用者，各自读到的是自己拥有的数据。

嵌入式调用者不在其外：它同样通过 executor 抵达引擎，因此它的操作按它传入的主体
校验。传 `Principal::unrestricted()` 是一句声明（"这是引擎自身的操作"，或"这个部署
主动退出"），而且写在了调用处。

## Snapshot 的 scope 归属

Snapshot target 由 `target` × `scope` 寻址（见[快照密封数据](./model/act.md)）。
角色的 `snapshot` 表限定该角色拥有哪些 target 的哪些 scope；`$subject`
会替换为角色名，所以 `["$subject"]` 表示"只读我自己的 scope"。

该规则在两处强制：

- `snap:*` 动作上——读写超出调用者集合的 scope 会被拒绝，`snap:ls`
  只返回该主体拥有的 scope，一个租户无法枚举他人的数据。
- **密封时**——流程带上启动它的那份规则，调度器在把 snapshot 值冻结进任务前会再校验
  一次。因此即使有人用别人的 `uid` 启动流程，工作流也读不到另一个主体已密封的数据。

流程的权威只有一个来源：用哪个主体创建 executor 启动它。子流程继承父流程的权威，
因此永远不可能读到开启它的那个流程读不到的东西。完全没有调用者的启动——`schedule`
触发器、嵌入式调用者直接调 `Runtime::start`——**不带**任何权威，也就读不到任何
snapshot scope：权威缺失不等于权威无限，需要受管数据的任务会在密封时带着它缺的那个
主体报错，而不是被塞给整个数据面。

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
- **订阅的积压是有界的。** 每个订阅者只有一个定长队列（`[grpc].queue_size`
  默认 128，`[web].queue_size` 默认 100）：投递不等待客户端，队列满即表示客户端
  已经停止读取，该订阅随即被断开——引擎不会为每条塞不进去的消息留一个等待发送的
  任务。引擎仍欠这个通道的消息不会随断开而消失：发出它的流程尚未结束、且尚未 ack
  的投递，会在客户端以同一个 id 重新订阅（落在同一个通道键上）后被重试定时器
  重新投递；与任何一次断线一样，通道只会收到它注册期间发出的消息。
- `msg:ack` 与 `msg:unsub` 同样是普通动作：拿到授权就能 ack 任意投递 id、取消自己
  命名空间内的任意通道。投递 id 不是按客户端寻址的，因此 `msg:ack` 应按"写入"级别
  授予——它的持有者可以压掉别的调用者尚未 ack 的投递。

## 各层各管什么

四类规则，各自在能表达它的那一层校验：

| 规则 | 作用对象 | 依据 |
| --- | --- | --- |
| 动作权限 | 每一个操作，同一张表 | 角色的 `allow`/`deny` 模式 |
| snapshot scope 归属 | `snap:*` 动作，以及密封时再校验一次 | 角色的 `snapshot` 表（`$subject`） |
| 通道命名空间 | 通道键与 `msg:unsub` | 已认证主体 |
| 目录限定 | 流程的 workdir | `[acl]`/角色的 `workdir` |

每一个操作都走动作表，这正是校验得以普遍的原因：传输层把 token 解析成主体，
嵌入式调用者同样如此——操作所运行的 executor 带着那个主体，而动作表与 executor
匹配的是同一批动作名，每个操作只定义一处。唯二**没有**身份可言的调用者，动作表
也为它们各自备好了答案：

- 不带 token 的请求落到 `default_role`；引擎没有 `[acl]` 段时落到只读的 `anonymous`
  主体——它仍然是一个调用者，因此照样被校验；
- 没人认领的启动（`schedule` 触发器）与子流程（继承父流程的权威）是引擎自身的启动，
  没有调用者可校验：它们能读什么，取决于最终带上的那份权威，而两者都无法从外面
  获得权威。


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

该包另有一层 `[shell]` 配置：`allow`/`deny` 两张 glob 清单，匹配**整段脚本文本**
（`*` 匹配任意字符，含 `/` 与换行），`deny` 优先；两张都为空表示不限制。不合法的
模式是启动错误，绝不静默放行。与目录检查一样，它是策略而非沙箱：文本 glob 看不到
脚本将做什么（`a=rm; $a -rf /` 里没有一个被禁的词），因此它用于把意图写明、把明显
的那一类拒掉，真正的边界仍然要靠操作系统。

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
