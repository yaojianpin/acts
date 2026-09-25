# 访问控制

引擎的每一个操作，都要拿执行它的那个调用者身份做一次校验。访问控制**始终开启**，
而且不再由配置文件定义：`acts.toml` 里残留的 `[acl]` 段会被忽略并记一条警告，
配置里没有 token、没有角色、也没有兜底角色。**用户存放在引擎的存储里、在运行期管理**
——用 `acl:*` 动作创建、授权、删除（其上就是 `acts-cli auth user …`），并且装了用户表的
引擎自带内置的 `admin`（裸引擎则没有任何用户；见[用户存放在哪](#用户存放在哪)）。

引擎认不出身份的请求——完全没带 token，或带了一个未知/过期的——就是**匿名**调用者：
它只能 list/get **模型与包**，没有别的读取、没有写入、没有控制类动作、没有管理类动作、
没有 snapshot scope、也不能订阅。未配置的部署是用来"看"已经部署了什么的，不是用来改它、
也不是用来读某个流程、某条消息或某个触发器里的内容。其余一切都要求先登录。

唯一的、写在代码里而非配置里的主动退出姿势是 `Engine::builder().disable_acl()`：
它让所有调用者不受限——开启 ACL 之前的行为，也是测试、演示或单租户嵌入式部署用的设置。
这是调用处的一句声明，而不是"没写某项配置"；此时登录会被拒绝，报
"the acl is disabled: requests need no login"。

任何地方都没有**隐式**的不受限策略：授权清单不全不是，没人认领的流程不是，
没人点名的 snapshot scope 也不是。它们各自对应下面写明的"什么都不读"的情形。

## 用户存放在哪

引擎把**策略**与**用户表**分在两个 crate 里：

- **`acts`** 持有引擎真正强制的东西。`acts::Principal` 是编译后的答案——它决定某个动作、
  某个 snapshot scope、某个资源名是否被允许；`acts::AccessControl` 是引擎与之对话的端口：
  `load`、`enabled`、`authenticate`、`anonymous`、`login`、`refresh`、`logout`、
  `set_user`、`del_user`、`user_names`、`get_user`。`Engine::acl()` 以
  `Arc<dyn AccessControl>` 交出已装上的那一份。`acts::UserPolicy` 是用户授权许可的普通
  数据形态，`Principal::from_policy` 负责把它编译成 principal。
- **`acts-acl`** 持有随发行版提供的实现：`acts_acl::UserAcl`，一个存放于存储中的用户表
  与会话表。装它只需一次调用：

```rust
use acts::Engine;
use acts_acl::AclUsers;

let engine = Engine::builder().with_user_acl().start().await?;
```

自行构造的用户表由 `EngineBuilder::set_acl` 装入，例如
`Engine::builder().set_acl(Arc::new(acts_acl::UserAcl::new()))`；该 trait 是公开的，
因此嵌入式调用者也可以装自己的实现。

**裸引擎**——什么都没装的 `Engine::builder().start()`——运行的是 `acts::AnonymousAcl`：
它不认识任何用户，所有调用者都是只管目录的匿名主体，`acl:login` 会被拒绝并报
"no user registry is installed in this engine"。这是刻意的默认，而不是漏洞：从未装过
用户表的嵌入式引擎没法登录，因此谁也无法变成别人。`acts-server` 在它的 engine builder 里
装入了 `acts_acl::UserAcl`，所以随发行版提供的服务端行为就是本章描述的样子。

`UserAcl` 在引擎自己的存储里用两个集合、各自的前缀保存数据：

| 前缀 | 一行代表 | 保存的内容 |
| --- | --- | --- |
| `acl_users` | 一个用户 | 名字（同时是主体）、加盐后的密码哈希（`salt$sha256(salt:password)`）、allow/deny/patterns/snapshot 授权、启用开关 |
| `acl_sessions` | 一次存活登录 | 该会话访问 token 与刷新 token 的 sha256 摘要，以及两者的过期时间（访问摘要就是行 id） |

它们就是普通的存储文档，走的是工作流行所用的同一个存储——同一个后端、同一套数据库租约与
写入分片——因此重启后用户与会话都还在。存储里只保存 token 的 sha256 摘要；明文只存在于
传输途中和客户端自己的会话文件里。

本章后面用到的名字与时长就是该 crate 的常量：`acts_acl::ADMIN_USER`（`admin`）、
`acts_acl::ADMIN_PASSWORD_ENV`（`ACTS_ADMIN_PASSWORD`）、
`acts_acl::ACCESS_TOKEN_TTL_SECS`（3600）与 `acts_acl::REFRESH_TOKEN_TTL_SECS`（604800）。

## 用户、密码与内置 admin

用户就是存储里的一行（`acl:setuser` 创建或更新，`acl:deluser` 删除，
`acl:getuser`/`acl:users` 读回）。`acl:setuser` 收的是 `acts::UserSpec`——字段就是下面这些，
其中 `None` 表示"该项保持不变"，清单字段则整体替换——写入的是一行 `acts_acl::AclUser`。
`acl:getuser` 返回策略本身，但不含密码哈希：name、`enabled`、`allow`、`deny`、`patterns`、
`snapshot`、密码条数，以及该用户是否不受限。一个用户带着：

| 字段 | 含义 |
| --- | --- |
| `passwords` | 一个或多个密码；每个都带独立盐做哈希（sha256）后存储，绝不存明文。存多个即可不停机轮换：先加新的，再删旧的。 |
| `allow` / `deny` | 决定该用户能执行哪些**动作**的命令模式与目录（catalog）模式（`deny` 优先）。 |
| `patterns` | 资源名（`rn`）模式，限定该用户能部署与启动哪些工作流。 |
| `snapshot` | 按 target 给出该用户拥有的 scope 模式。 |
| `enabled` | 被禁用的用户无法登录，其存活会话一并失效。 |

引擎总是自带内置管理员 `admin`：

- 它不受限（所有动作、所有 snapshot scope、所有资源）；
- 它的密码在全新存储的首次启动时取自 `ACTS_ADMIN_PASSWORD`；未设置该变量时随机生成并
  **只在日志里打印一次**——之后可用 `acl:setuser` 修改；
- 它不能被删除或禁用，因此引擎永远不会把自己锁在自己的用户表之外。
  `acl:deluser admin`、`enabled = false` 的 spec、以及
  `acts-cli auth user set admin --disable` 都会被拒绝。

只有拿到相应授权的用户能改动用户表：`acl:setuser` 与 `acl:deluser` 属于 `@write`
分组，`acl:getuser`/`acl:users` 则是读取（`@read`）——它们都是普通动作，照常校验。读写密码
不是传输层能做的事——密码只会作为 `acl:login` 的载荷出现。

用户名同时是引擎归属工作的**主体**，也是该用户所开每条订阅的通道键前缀，因此含有路径
分隔符（`/`）的名字在写入用户时就被拒绝。

请求是按用户**当前**那一行解析的，而不是登录时取的副本，因此增删授权会在存活会话的
下一个请求上生效；被禁用的用户立即不再认证（其 token 解析为 `anonymous`），
`acl:deluser` 会吊销被删用户的全部会话。

## 登录与会话

调用者通过登录成为某个用户：

```jsonc
// acl:login {user, password}
{"token": "…", "refresh_token": "…", "expires_in": 3600, "refresh_expires_in": 604800}
```

- **访问 token**（`expires_in`，一小时）以 `authorization: Bearer <token>` 凭证随每个
  请求下发。
- **刷新 token**（`refresh_expires_in`，七天）在访问 token 过期后交给
  `acl:refresh` `{refresh_token}`；返回一对新的，并且**旧的一对随轮换作废**——
  刷新 token 是一次性的，泄露了也无法重放。
- `acl:logout` `{token}` 吊销该 token 所属的会话（访问或刷新任一形态），返回
  `true`/`false`。
- `acl:login`、`acl:refresh`、`acl:logout` **不需要任何授权**：凭据本身就是载荷，
  而登出只会吊销调用者自己的凭据。
- token 是不透明的随机串；存储里只保存它们的 sha256 摘要，因此明文只存在于传输途中和
  客户端自己的会话文件里，服务端不留。
- `acl:whoami` 同样不需要授权：它返回服务端为本次请求解析出的身份——`{user, subject,
  authenticated, unrestricted, allow, deny, patterns, scopes, workdir_root}`。匿名调用者
  也会得到应答，只是 `"authenticated": false`——正因如此它可以当启动自检用。
  `workdir_root` 是该主体自己的文件系统根：它是预留字段，恒为 `null`，因为目录根是引擎的
  全局 `workdir` 设置（见[流程目录](#流程目录)）。

## 授权

用户的 `allow` / `deny` 清单里有两类 token：

- **命令模式**——共享动作表里动作名上的 glob（`*`、`?`），所以 `model:*` 覆盖全部模型
  操作，`act:*` 覆盖全部 act 操作。这些名字与 CLI、channel 客户端使用的完全一致。
- **目录引用**——`@read`、`@deploy`、`@execute`、`@write` 指定动作所属的分组，
  `@all`（或 `@*`、`*`）指定全部分组。即 Redis ACL 里 `+command` 与 `+@category` 的形态。

`deny` 优先于 `allow`，并且默认**拒绝**：没人授予的动作一律不放行；没有任何分组认领的
动作算 `write`——未知操作永远不会被当成读取。写入用户时，点名不到任何分组目录 token
（分组就是 `read`、`write`、`deploy`、`execute`、`all`，清单即 `acts::CATALOG_GROUPS`）
会被直接拒绝，因此写错的分组会大声报错，而不是静默地什么都不授予、也什么都不拒绝。

### 动作分组

| 分组 | 动作 |
| --- | --- |
| `@read` | `model:ls` `model:get` `pack:ls` `pack:get` `proc:ls` `proc:get` `task:ls` `task:get` `msg:ls` `msg:get` `evt:ls` `evt:get` `snap:get` `snap:ls` `msg:sub` `acl:whoami` `acl:getuser` `acl:users` |
| `@deploy` | `model:deploy` `pack:publish` |
| `@execute` | `proc:start` `proc:start_from_model` `evt:start` `act:push` `act:remove` `act:submit` `act:complete` `act:abort` `act:cancel` `act:back` `act:skip` `act:error` `msg:ack` |
| `@write` | `model:rm` `pack:rm` `msg:rm` `msg:redo` `msg:clear` `msg:unsub` `snap:upsert` `snap:remove` `acl:setuser` `acl:deluser` `ext:register_var`，另有 `acl:login` `acl:refresh` `acl:logout`（这三个本就不需要授权） |
| `@all` | 全部动作（`@*` 与 `*` 是同一份授权） |

这四个分组对应一次部署通常要划开的四种权限：`@read` 只看，`@deploy` 把工作放进目录，
`@execute` 运行它、并通过 act 推进它，`@write` 改动或销毁已存状态——删除、snapshot 写入，
以及**用户表本身**（`acl:setuser`/`acl:deluser`），因此授予 `@write` 就等于授予管理权。
没有任何分组点名的动作按 `write` 处理——未知操作永远不会被当成读取。

授予其中一个分组绝不隐含另一个：`@execute` 能启动、能推进流程，但既不能部署、也不能删除；
而 `@write` 不能让用户跑任何流程。`allow` 里的 `@all` 使该用户不受限——所有动作放行、
所有 snapshot scope 可读、任何资源名都可部署；`deny` 里的 `@all` 则拒绝一切，压过任何
本可放行的命令模式。

`ext:register_var` 是嵌入式调用者自己的接口——把用户变量模块装进表达式环境。线上动作表里
没有任何一项通到它，因此传输层的调用者根本够不着；扩展自己所托管的引擎的嵌入式调用者，
传的是它自己的身份（见[executor](#executor)）。发布包定义（`ext().register_package`）
走的是 `pack:publish`，因此随 `@deploy` 走。

`anonymous` 主体拿到的正是 `model:ls` `model:get` `pack:ls` `pack:get`——目录。
这是引擎无法指名道姓的调用者可以被授予的最小集合，而且这份清单由测试钉死，不留给解读。
其余全部不在其中，包括命名调用者看来理所当然的读取：一条流程行指名了谁跑了什么，
一条投递指名了它本是发给谁的，一个触发器指名了它将启动哪个模型——引擎认不出的调用者，
不是这些行所描述的那个人。`msg:sub` 同理，还多一条理由：一条订阅流既承载实时载荷，
又会为它持有的通道逐条写入投递行；`snap:get`/`snap:ls` 也不在其中，因为 snapshot scope
只有在用户点名时才有归属。

### 资源模式

一个用户能**部署**和**启动**哪些工作流，与它能跑哪些动作是分开决定的：工作流用 `rn`
声明自己操作的资源，那是以冒号分隔的字面名（`orders:eu`，不含空格、不含 glob 字符、
不含空段）；每个用户有一份 `patterns`，其 `rn` 必须命中其中之一。

```yaml
name: order flow
id: order-eu
ver: 0.1.0
rn: orders:eu
steps:
  - name: pick
    uses: acts.core.set
    params:
      message: "hello"
```

授予了 `--pattern 'orders:*'` 的用户可以部署、启动该模型；只授予
`--pattern 'orders:us'` 的用户不行，拒绝信息会点名资源
（`resource 'orders:eu' is not allowed for user 'alice'`）。`rn` **为空**的模型不声明
任何资源，因此**只有不受限用户**才能部署或启动它——不声明不等于声明了一切。该校验在
部署时、流程启动时、触发器启动时各做一次，所以无论是线上动作还是直接驱动模型的嵌入式
调用者，都受同一条约束。

### Snapshot scope

`snapshot` 字段是按 target 给出的"target → scope 模式"表；模式里的 `$subject` 会替换成
用户名，所以 `["$subject"]` 表示"只读我自己的 scope"，`["$subject/*"]` 表示"我名下的
那些 scope"。

```jsonc
// acl:setuser {user: {name: "alice", allow: ["@read", "snap:upsert"],
//                     snapshot: {"secrets": ["$subject"], "profile": ["$subject/*"]}}}
```

表里没写到的 target 完全不属于该用户；不受限用户拥有一切。

## executor

引擎的操作收在同一个对象上，即 executor，它**每个方法都在运行前先做一次校验**——
`model().deploy()`、`proc().start()` 以及其余全部。创建时就绑定了调用者：

```rust
// 传输层的请求：token 解析完之后
let executor = engine.executor(&principal);
executor.proc().start("my_model", vars).await?;

// 没带可用 token 的调用者
let executor = engine.executor(&engine.anonymous());

// 引擎自身的操作；测试与本地演示也传这个
let executor = engine.executor(&Principal::unrestricted());
```

executor 还决定它启动的流程**带上什么**：`proc().start()` 与 `evt().start()`
把该主体的 snapshot scope 与资源模式封进流程，因此调用者无法通过在请求里塞一个权威来
放宽自己的读取范围。同一个模型的两个调用者，各自读到的是自己拥有的数据；用户在不拥有的
`rn` 上启动流程，会在流程存在之前就被拒绝。

嵌入式调用者不在其外：它同样通过 executor 抵达引擎，因此它的操作按它传入的主体校验。
传 `Principal::unrestricted()` 是一句声明（"这是引擎自身的操作"，或"这个部署主动退出"），
而且写在了调用处。

## Snapshot 的 scope 归属

Snapshot target 由 `target` × `scope` 寻址（见[快照密封数据](./model/act.md)）。
用户的 `snapshot` 表限定该用户拥有哪些 target 的哪些 scope；`$subject`
会替换为用户名，所以 `["$subject"]` 表示"只读我自己的 scope"。

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

- **订阅是一个动作。** 打开一条流（gRPC `on_message`、SSE `/msg/sse`）需要用户的
  `allow` 里含 `msg:sub`；不含时传输层直接回 `PERMISSION_DENIED` / `403`，
  不发流。传输层把收到的 client id 交给该动作，并用动作返回的键注册通道，因此
  "被校验的路径"与"实际占用的键"不可能各走各的。
- **键按主体划分命名空间**：`{subject}/{传输自身的 id}`——用户名作前缀，后面接传输
  自己的 client id（SSE 保留自己的 `acts-flow-client-` 段）。因此第二个调用者用
  别的主体已经用过的 client id 订阅时，得到的是**另一个**通道，而不是顶替那个主体
  的 handler。`msg:unsub` 由同一个 id 组合出同一个键，所以调用者只能取消自己命名
  空间里的通道；用户名含 `/` 会在写入用户时被拒绝，前缀的无歧义正是靠这一点保证。
- **投递只看过滤器与授权**：通道会收到所有匹配它自报的 `type`/`state`/`uses`/
  `options` glob 的消息，无论发出它的流程是谁启动的；拿到消息后能做什么，由调用者
  持有的动作决定。不希望读到消息载荷的用户，就是没有 `msg:sub`（以及没有
  `msg:ls`/`msg:get`）的用户。
- **订阅的积压是有界的。** 每个订阅者只有一个定长队列（`[grpc].queue_size`
  默认 128，`[web].queue_size` 默认 100）：投递不等待客户端，队列满即表示客户端
  已经停止读取，该订阅随即被断开——引擎不会为每条塞不进去的消息留一个等待发送的
  任务。引擎仍欠这个通道的消息不会随断开而消失：发出它的流程尚未结束、且尚未 ack
  的投递，会在客户端以同一个 id 重新订阅（落在同一个通道键上）后被重试定时器
  重新投递；与任何一次断线一样，通道只会收到它注册期间发出的消息。
- `msg:ack` 与 `msg:unsub` 同样是普通动作：拿到授权就能 ack 任意投递 id、取消自己
  命名空间内的任意通道。投递 id 不是按客户端寻址的，因此 `msg:ack`（属于 `@execute`）
  应按写入的谨慎程度授予——它的持有者可以压掉别的调用者尚未 ack 的投递；`msg:unsub`
  属于 `@write`。

## 各层各管什么

四类规则，各自在能表达它的那一层校验：

| 规则 | 作用对象 | 依据 |
| --- | --- | --- |
| 动作权限 | 每一个操作，同一张表 | 用户的 `allow`/`deny` 模式（命令 glob 与 `@`目录分组） |
| snapshot scope 归属 | `snap:*` 动作，以及密封时再校验一次 | 用户的 `snapshot` 表（`$subject`） |
| 资源归属 | `model:deploy`、`proc:start`、`proc:start_from_model`、`evt:start` | 用户对工作流 `rn` 的 `patterns` |
| 通道命名空间 | 通道键与 `msg:unsub` | 已认证主体（用户名） |

每一个操作都走动作表，这正是校验得以普遍的原因：传输层把 token 解析成主体，
嵌入式调用者同样如此——操作所运行的 executor 带着那个主体，而动作表与 executor
匹配的是同一批动作名，每个操作只定义一处。唯二**没有**身份可言的调用者，动作表
也为它们各自备好了答案：

- 没带可用 token 的请求就是匿名——它仍然是一个调用者，因此照样被校验：目录之外的动作
  会被按 `unauthenticated`（请先登录）拒绝，而不是 `permission denied`；
- 没人认领的启动（`schedule` 触发器）与子流程（继承父流程的权威）是引擎自身的启动，
  没有调用者可校验：它们能读什么，取决于最终带上的那份权威，而两者都无法从外面
  获得权威。

## 流程目录

流程目录的根是一个**全局引擎设置**，不属于任何用户：`workdir = "/srv/acts"`
（`acts.toml` 顶层，`Config::workdir()`）。它是一个**根目录**：每个流程得到它下面的
自己的目录 `<workdir>/<pid>`，文件系统访问被限定在那里；进程 id 成为路径的一段——
因此不能作为单个安全目录名的 pid（空、`.`、`..`，或含路径分隔符、冒号）会被拒绝，
而不是被放到根目录之外。`workdir` 为空是配置错误，而不是"不做目录限定"。

根目录走与 scope 权威同一条私有通道——它本身就是权威的一部分：随 owner 权威封入进程
env（`$env` 代理拒绝该私有键），随进程持久化，且不作为启动参数下发，因此调用者无法指定
流程被限定到哪个目录。**流程自己的目录**（即 `<root>/<pid>`）由三处读到：act 用
`Context::workdir()`，工作流脚本用 `$env.WORK_DIR`，两者指的是同一个目录，且都不是配置
里写的那个根。`$env.WORK_DIR` 由引擎应答，写入会被丢弃，脚本无法改写自己所在的目录。

目录的生命周期与进程的持久行一致：进程结束且消息投递结清后，sweeper 删除行时一并
删除目录（投递出错、等待人工重投的流程保留行，目录也随之保留）；从未落盘的启动则
立即删掉自己的目录。因此留在目录里的文件不会比流程本身活得更久。

`acts.app.shell` 把它挂载为脚本文件系统的**根**：脚本运行在 bashkit 的虚拟 bash 里，
`pwd` 就是 `/`，相对路径落在本次流程的目录内，且 `/` 是脚本唯一能命名的目录树——
宿主机的其余部分根本不在它拿到的文件系统里。两个名字指的是同一批文件：宿主放进该
目录的文件，脚本在同一相对路径上读到；脚本写进去的文件，宿主也看得到。`HOME`、
`TMPDIR`/`TEMP`/`TMP`、`PWD` 都指向根，`ACTS_WORKDIR` 让脚本能直接引用它（`/`）。

该包另有一层 `[shell]` 配置：`allow`/`deny` 两张 glob 清单，匹配**整段脚本文本**：

```toml
[shell]
# 非空时，只有命中其中一条的脚本才能运行
allow = ["ls", "ls *", "cat *.txt"]
# 无论 allow 如何，一律拒绝
deny = ["*rm -rf*", "*sudo *"]
```

`*` 匹配任意字符，含 `/` 与换行，`deny` 优先；两张都为空表示不限制。不合法的
模式是启动错误，绝不静默放行。这一层是策略而非沙箱：文本 glob 看不到脚本将做什么
（`a=rm; $a -rf /` 里没有一个被禁的词），因此它用于把意图写明、把明显的那一类拒掉；
脚本真正能碰到什么，由它拿到的文件系统决定——那只有本次流程自己的目录，没有宿主机
的其他任何部分。`shell: bash` 是唯一允许的解释器：包运行的是 bashkit，不是
PowerShell、Nushell 或 POSIX `sh`，`params` 里写了别的值会在读取参数时就失败。

不配置 `workdir` 时不挂载任何宿主目录，shell act 改用解释器自身的内存文件系统：
写入会成功，但那个文件系统背后没有宿主机，流程不会在宿主上留下东西，也同样读不到
宿主上的任何文件。

## 各传输的凭证

| 传输 | token 的携带方式 |
| --- | --- |
| gRPC | 每次请求的 `authorization: Bearer <token>` metadata，包括 `on_message` 订阅 |
| HTTP | `authorization: Bearer <token>` 请求头；`/health` 保持开放以便探活 |
| NATS | 动作 JSON body 里的 `token` 字段——broker 认证的是连接，不是单次请求 |

拒绝分两种：`unauthenticated`（没有可用的会话 token）与 `permission denied`
（有会话，但没有该操作的权限）；gRPC 回 `UNAUTHENTICATED` / `PERMISSION_DENIED`，
HTTP 传输回 `401` / `403`。

## 客户端

CLI 负责登录、保存会话，并管理用户表：

```bash
# 登录并保存会话（密码取自 ACTS_PASSWORD，否则交互提示）
acts-cli auth login alice
ACTS_PASSWORD=s3cret acts-cli auth login alice

# 查看服务端为本会话解析出的身份，然后退出登录
acts-cli auth whoami
acts-cli auth logout

# 用户表（set/rm 需要 @write 授权：其余人一律由服务端拒绝）
acts-cli auth user ls
acts-cli auth user get alice
acts-cli auth user set alice \
    --allow @read --allow @deploy --allow @execute \
    --deny 'model:rm' \
    --pattern 'orders:*' \
    --snapshot 'secrets=$subject' --snapshot 'profile=$subject/*' \
    --password s3cret
acts-cli auth user set alice --rm-password old-secret --disable
acts-cli auth user set alice --enable
acts-cli auth user rm alice
```

- `--allow`/`--deny` 取上面那两类模式（命令模式与 `@`目录分组）并**整体替换**清单；
  `--pattern` 设定资源模式（可重复给出；该清单整体替换旧的）；`--snapshot` 取
  `TARGET=GLOB[,GLOB]`；`--password`
  追加一个密码，`--rm-password` 按明文删除一个密码；`--disable`/`--enable` 切换启停
  （禁用期间登录被拒，存活会话失效）。
- 每条 `auth user …` 都以会话自身的身份发出：`set` 与 `rm` 需要 `@write` 授权，
  `ls`/`get` 需要 `@read` 授权。没有相应授权的调用者由服务端拒绝，客户端从不代为判断。
- 写在命令行上的密码对本机进程列表可见；`ACTS_PASSWORD`（以及省略时的交互提示）可以让
  它不出现在那里。

启动时 CLI 会优先复用该服务端的**已存会话**，因此不必每次登录：

```bash
# 显式 token 原样使用——它属于调用者，不该被揣测
acts-cli --token "$TOKEN"
ACTS_TOKEN="$TOKEN" acts-cli

# 否则：进入 REPL 前先用该用户登录
acts-cli --user alice --password s3cret
ACTS_USER=alice ACTS_PASSWORD=s3cret acts-cli
```

会话保存在 `$ACTS_CONFIG_DIR`（否则 `$HOME/.acts`，Windows 为 `$USERPROFILE`，
再否则 `./.acts`）下的 `session.json`；在 unix 上以仅属主可读写权限写入，
`auth logout` 会删除它。已存的访问 token 过期时，会在第一个请求上用它带的刷新 token
轮换；无法再认证的会话在有用户名密码时由一次 `acl:login` 修复，否则被丢弃。进入 REPL 前
CLI 先用 `acl:whoami` 解析出身份并打印，因此凭证缺失或过期会在启动时就暴露，而不是拖到
第一条命令：管理员打印 `authenticated as alice (unrestricted)`，普通用户打印
`authenticated as alice`；完全没有建立会话时打印

```text
connected anonymously to http://127.0.0.1:10080: the catalogue reads are available, nothing else. Log in with 'auth login <user>' or --user.
```

```rust,no_run
use acts_channel::ActsChannel;

// 连接并在每个请求上携带 token，过期时自动刷新
let mut client = ActsChannel::connect_with_session("http://127.0.0.1:10080", session).await?;

// 或者：用用户名密码登录，不保存任何东西
let mut client = ActsChannel::connect_with_password("http://127.0.0.1:10080", "alice", "s3cret").await?;
```

`connect` 是不带 token 的形式，即匿名调用者：它只能读目录，别的什么都不行。未认证的
调用者会被按 `unauthenticated` 拒绝，而不是 `permission denied`——这正是客户端据此
判断"该去登录"而不是"该求更多授权"的依据。
