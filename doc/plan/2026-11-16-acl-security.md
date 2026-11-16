# ACL 安全加固实施计划

- 日期:2026-11-16
- 输入:`doc/design/acl-security.md`(定稿设计,含 0.x 不迁移、引导窗口两项用户决策)
- 范围:`crates/acts`(store 集合层、acl trait、动作门禁、LoginTokens)、`crates/acts-acl`、`plugins/acts-plugin-web|grpc|nats`

## 阶段与任务

### P0 — 集合层条件写原语(schema 无关)

| # | 任务 | 位置 |
| --- | --- | --- |
| 0.1 | `create` 改 insert-only、新增 `upsert`、`update_guarded` CAS(无跨文档批) | `crates/acts/src/store/mod.rs`(trait)、`collection.rs`(KvCollection 实现) |
| 0.2 | 实现:`lock_docs` 全部文档键 → 逐文档组 ops(Put 带 expected 行的索引差分 / Delete 带 expected 行索引清除)→ 一次 `kv.batch(ops, guards)`;guard 失配整批不落 | `collection.rs` |
| 0.3 | 单测:create 二次拒绝且不覆盖、upsert 重建索引、update_guarded 过期 expected 冲突且行不变 | `mod.rs` tests(现有 `Row` 夹具 + MemoryStore) |

验收:`cargo test -p acts --lib store::` 通过。

### P1 — 凭据止血(v0 schema 上原地)

| # | 任务 | 位置 |
| --- | --- | --- |
| 1.1 | workspace 增 `argon2 = "0.5"`;acts-acl 引入 | 根 `Cargo.toml`、`acts-acl/Cargo.toml` |
| 1.2 | Argon2id PHC 编码/验证(参数 env 可调 `ACTS_ARGON2_M_KIB/_T/_P`,默认 19456/2/1)、dummy 验证(LazyLock 固定输入)、>1KiB KDF 前拒、MAX_PASSWORDS=3;删除 `salted()/verify_password()` | `acts-acl/src/lib.rs` |
| 1.3 | 登录预算:KDF 全局 Semaphore(默认 8)、每用户名失败指数退避(LRU 上限 10k,5 次起 1s→900s)、每主体成功令牌桶(10/min burst 5) | `acts-acl/src/lib.rs` |
| 1.4 | Bootstrap:env 直通;无 env 时 `DEFAULT_ADMIN_PASSWORD` + 初始状态标记;`AccessControl::password_change_required()` | `acts-acl/src/lib.rs`、`crates/acts/src/acl.rs` |
| 1.5 | 初始状态门禁:仅允许自改密;改密清空旧凭据 + 清标记 + security_version+1 → 强制重登 | `acts-acl/src/lib.rs`、`crates/acts/src/export/actions.rs` |
| 1.6 | 动作门禁:`apply_as` 在 `check_action` 前短路放行窗口内 `acl:setuser`(目标 admin、add_passwords 非空) | `crates/acts/src/export/actions.rs:164` 附近 |
| 1.7 | (取消)默认密码方案下无需 loopback 门禁 | — |
| 1.8 | `LoginTokens` 手写 `Debug`,token 只露前缀+长度 | `crates/acts/src/acl.rs:182` |
| 1.9 | acts-acl 测试:默认密码初始状态全路径、PHC 验证、>1KiB、上限、Debug 不泄 token | `acts-acl/src/lib.rs` tests |

验收:`cargo test -p acts-acl --lib`、`cargo test -p acts --lib store:: export::` 通过;clippy/fmt 干净。

### P2 — 身份与会话代际(v1 schema,不迁移)

| # | 任务 | 位置 |
| --- | --- | --- |
| 2.1 | v1 结构:`AclUser`(+subject_id/security_version/document_revision/max_sessions/PasswordCredential)、`AclSession`(+session_id/family_id/parent/subject/security_version/state/revision,refresh 等值索引 + refresh_expiry 有序索引)、`AclTombstone`、`SessionState`;version=1,upcast 只认 v1 | `acts-acl/src/lib.rs` |
| 2.2 | 认证路径:本地 session 缓存 miss 回源 `find_opt`;subject/security_version 比对;Principal 按 document_revision 复用 | `acts-acl/src/lib.rs` |
| 2.3 | refresh:guard CAS 消费旧行 → 插入 successor(新 id);replay 检测 + 清族 | `acts-acl/src/lib.rs` |
| 2.4 | `del_user` 原子批(删用户行 + tombstone,guard 用户行字节);会话零遍历即死 | `acts-acl/src/lib.rs` |
| 2.5 | `set_user` 读 Store 权威行 + `update_guarded`;`ActError::Conflict` 新变体 | `acts-acl`、`crates/acts/src/error.rs` |
| 2.6 | 三态 auth:`AuthenticationResult` + `AccessControl::resolve`(默认实现包旧语义);web→401、gRPC→unauthenticated、NATS→拒绝回包 | `crates/acts/src/acl.rs` + 三 transport |
| 2.7 | transport 错误映射:Conflict→409/aborted | 三 transport |
| 2.8 | 测试:64 路并发 refresh 恰 1 成功、双实例 sqlite 共库、删除同名重建隔离、并发 set_user CAS、store 错误注入无双后继 | `acts-acl` tests |

验收:`cargo test -p acts-acl --lib` 通过;`cargo test -p acts-plugin-web --lib`、`-p acts-plugin-grpc --lib`、`-p acts-plugin-nats --lib`(NATS 需本地 JetStream,缺席明示)。

### P3 — 容量与多实例

| # | 任务 |
| --- | --- |
| 3.1 | 每用户会话配额(默认 10,UserSpec.max_sessions 覆盖;超限同批淘汰最老 Active) |
| 3.2 | GC 循环(load 末 spawn,refresh_expiry 有序分页,滞留 1h,批 500) |
| 3.3 | 启动加载改造:分页遍历只物化 Active |
| 3.4 | SecurityHeader / session 缓存 TTL + fail-closed(`ACTS_ACL_HEADER_TTL_MS`/`ACTS_ACL_SESSION_TTL_MS`/`ACTS_ACL_FAIL_CLOSED`) |
| 3.5 | (可选,缺省不做)NATS 失效广播 |

验收:配额淘汰、GC 分页、双实例收敛窗口测试。

### P4 — 运营收尾

| # | 任务 |
| --- | --- |
| 4.1 | tracing 结构化指标字段(acts_acl_* 命名,见设计 §20) |
| 4.2 | `crates/acts-acl/README.md` 重写(bootstrap 三路径、密码参数、预算)、CHANGELOG 条目 |

## 统一门禁(每阶段交付前)

```sh
cargo clippy --all-targets --all-features --workspace --locked -- -D warnings
cargo fmt
cargo test -p acts --lib            # 阶段末跑全量,过程中按模块过滤
cargo test -p acts-acl --lib
cargo test -p acts-plugin-web --lib
cargo test -p acts-plugin-grpc --lib
```

NATS/Postgres 依赖的套件本地缺席时如实标注未跑(CI 补齐)。

## 进度

- [x] P0(2026-11-16:create=insert-only / upsert / update_guarded,单测绿)
- [x] P1(2026-11-16:Argon2id + dummy、登录预算、默认密码初始状态、门禁)
- [x] P2(2026-11-16:v1 schema、状态机、refresh CAS 定序、三态 auth,26/26)
- [x] P3(2026-11-16:配额、GC、TTL 缓存、fail-closed)
- [x] P4(指标/README/CHANGELOG 完成;门禁全绿:clippy/fmt/acts 1818/doc/插件/store/cli/server)

用户决策(2026-11-16,已落地):bootstrap 改为"默认密码 admin + 初始状态";
集合层 API 改为 create=insert / upsert / update_guarded,撤销 write_batch_guarded
(ACL 局部以 CAS 定序实现)。
