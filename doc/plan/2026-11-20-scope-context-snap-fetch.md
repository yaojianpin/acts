# 2026-11-20 业务授权上下文与 Snapshot 拉取改造

依据：`design/scope-context-and-snap-fetch.md`。

## P1 授权上下文（context）进入 ACL 与封存

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | `UserPolicy.context: BTreeMap<String,String>` + `SetUserPolicy.context`；key/value 校验（`[a-z][a-z0-9_]{0,31}`、值非空 ≤128、≤16 条）；acts-acl 行存储透传 | setuser 往返保持 context；非法 key/value 被拒；旧行（无字段）照读 |
| 2 | `substitute(pattern, subject, context)`：`$subject` + `$ctx.<key>`；scope 与 rn 两处调用点同步 | `$ctx.tenant/$ctx.unit/*` 命中；缺 key 的模式匹配任何值（fail-closed 测试） |
| 3 | `ScopePolicy.context`（serde default）随 `#owner` 封存；`scope_policy()` 编译携带；子流程经 `start_as_owner(owner_scope())` 继承 | 双租户同模型互不可读对方 scope；子流程继承；父策略变化不影响在跑实例 |
| 4 | `acl:whoami` 回显 `context` | whoami 输出含 context |
| 5 | 扩展伪造负向测试：`#owner` 含 context 的序列化体仍拒读 | 既有 forge 测试加 context 变体通过 |

## P2 Snapshot fetch-on-miss（内核）

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | `SnapshotOptions.fetch: Option<FetchOptions{timeout_ms}>`，注册时钳位 [100,30000]；builder 增设方法 | default None 行为与今天一致（纯推回归）；非法 timeout 被拒 |
| 2 | 提供者发布即注册：`pack:publish` 读 `options.var`，自动注册同名 target（fetch on，默认 timeout/TTL，options 可覆盖）；同 var 二次发布校验参数兼容，冲突拒绝；scope key 默认封存上下文 | 发布 `acts.clientA.profile` 后 `$profile` 可拉取；TTL 冲突发布被拒；未授权主体（无 target 模式）deny-all |
| 3 | `SnapFetchRegistry`：`(target,scope)` 单飞槽 + watch 通知；leader 发事件、有界等待；超时唤醒所有等待者为 `None` | 并发 prepare 同 scope 只发一次 req；超时全部按 on_missing |
| 4 | `resolve_sealed` 未命中分支接 registry：应答 → `SnapshotManager::upsert` → 重读 → seal；失败落 on_missing；`owner.allows` 检查保持在 fetch 之前 | 命中不发请求；回填后 seal 成功；未授权 scope 在 fetch 前被 `Denied`（顺序测试） |
| 5 | `KeyEvent::Req` + `Emitter::emit_req`：只进 `DeliveryTargets::matching` 的通道面，不落 Message/Delivery 存储，不触发 act 语义 | req 不出现在 msg:ls/delivery；`is_act()` 相关回归通过 |

## P3 线协议 req/reply 与应答 action

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | 线协议零改动：req 用 `OnMessage` 流消息（`seq`=引擎消息 id 作 req_id，`data.type="req"`），rep 用既有 `Send`/NATS `acts.cmd` 回 action `snap:reply`（seq 回显确认）；acts-proto 只补编解码/关联回归测试 | 三传输 wire 回归；req/rep 往返正确 |
| 2 | `apply_as` 新增 `snap:reply`：`principal.check_scope(target, scope)` → `SnapshotManager::upsert` → 按 `req_id` 唤醒 registry；归 `@write`（与 snap:upsert 同类） | 无授权应答被拒且不回填、等待者超时；过期 rev no-op 不回滚 |
| 3 | 传输接线：gRPC `Send{name:"snap:reply"}` 已天然路由到 `apply_as`（零改动）；NATS 同构（零改动）；Web 照 `snap_upsert` 加 `POST /api/snap/reply` handler（`/api/act/{action}` 白名单只映射 `act:*`，不可复用） | 三传输各自 e2e：表达式读 → req → rep → seal |
| 4 | 示例：examples 增加提供者 client（`pack:publish {id: "acts.myclient.profile", options.var: "profile"}` → 订阅 `uses:"profile"` + options 过滤 → 应答 `snap:reply`） | 示例可跑通一次冷启动拉取并命中缓存；同名双提供者各自只答自己租户 |

## P4 加固与运营

| # | 改动 | 验收 |
| --- | --- | --- |
| 1 | req 投递引擎侧过滤：按订阅主体 `check_scope(target, scope)` 决定是否投递（开放问题 §7.1，代价评估后做） | 无授权订阅者收不到 req 事件 |
| 2 | tracing/metrics：`snap_fetch_request/timeout/denied_total`，事件含 target/scope、不含数据 | 字段齐备，无 payload 泄露 |
| 3 | 文档：`snapshot.rs` 模块头、README access-control 章节、CHANGELOG | fetch 语义与不变量弱化写明 |

## 测试

- 单元：context 校验与替换（含 fail-closed）、ScopePolicy 序列化兼容、fetch
  单飞/超时/rev 守卫、`snap:reply` 授权、proto kind 编解码。
- 集成：双租户 seal 隔离；子流程继承；P3 表的传输 e2e（stub 提供者）。
- 回归锚点：`#owner` 伪造测试（含 context 变体）、`resolve_sealed` 的
  Denied/on_missing 既有测试、纯推模式零变化。
- 统一运行 AGENTS.md 门禁与 per-crate 套件；P3 触及 acts-server/插件 lib 测试，
  需要 NATS (127.0.0.1:4222)，缺失时按说明注明未跑项；资源受限加 `-j 2`。
