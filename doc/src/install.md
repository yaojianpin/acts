# 安装

`acts` 是一个快速、轻量、可扩展的工作流引擎库，使用 YAML 格式定义工作流并通过消息驱动架构执行和分发消息。

## 安装 acts 库

通过 `cargo` 命令安装：

```bash
cargo add acts
```

从源码构建需要 Rust 1.88 及以上版本：这是每个 crate 承诺的下限（各 manifest 中的
`rust-version`，由工作区统一继承），也是当前依赖图本身的要求。CI 用该下限
（`1.88.0`）和 `stable` 各构建一次，`rust-toolchain.toml` 则为本地检出选择 stable。

## 安装外部存储

外部存储后端（sqlite/postgres/sled）在独立的 `acts-store` crate 中，
启用对应 feature 后从 `acts_store` 导入后端，再通过 `EngineBuilder::set_store`
指定（不设置时默认使用内存存储 `MemoryStore`）：

```bash
# SQLite
cargo add acts-store --features sqlite

# PostgreSQL
cargo add acts-store --features postgres

# Sled
cargo add acts-store --features sled
```

```rust
use acts::Engine;
use acts_store::SqliteStore; // 或 PostgresStore / SledStore
use std::sync::Arc;

#[tokio::main]
async fn main() -> acts::Result<()> {
    let store = SqliteStore::open("data/acts.db").await?;
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .start()
        .await?;
    Ok(())
}
```

`acts` 本身只内置 `MemoryStore` 与自定义存储所需的 `KvStore` trait；实现
`acts::KvStore` 的自定义后端同样通过 `set_store` 注入。

每个数据库只能有一个写入者，这一点现在由引擎强制保证：`acts-server` 会在启动引擎
之前获取数据库上的**排他租约**（`[db] lease = true`，默认开启）。租约本身就是数据库
中的一行（`__acts_lease__`），用原子 compare-and-swap 抢占，按
`[db] lease_renew_secs` 周期续约；引擎的每一次写入都只在该实例仍持有租约时提交：

- 同一数据库上的第二个服务器启动会以 `LeaseHeld` 失败（并指明持有者），而不是再起一个
  会重复恢复与调度同一批行的引擎；
- 租约被抢占的实例（续约失败超过 `[db] lease_ttl_secs`，被别的实例接管）之后每次写入
  都会以 `LeaseLost` 被拒绝，引擎同时停止，因此它永远无法覆盖新持有者的行；
- 持有者的 **fence**（围栏令牌）每次获取都会递增（跨崩溃、跨重启），携带过期 fence 的
  写入会在**本应提交它的那次原子写入内部**被拒绝，而不是靠一个仍可能被别的进程抢先的
  前置检查。

这正是引擎自身锁覆盖不到的地方：保证文档行与索引行一致的文档锁（`DOC_LOCKS`）是进程内
的——同一进程内的所有引擎共享一张锁表，但两个进程各持一张——没有租约时，两个进程可能
同时读到同一行、各自算出要写的索引行并都提交，导致查询命中已不再持有的值。
租约建立在一个原语之上：`KvStore::batch(ops, guards)` 把 ops 与「所有守卫仍然成立」这一
条件作为后端的同一个原子步骤提交。守卫就是调用方当初据以决策的已存状态，它是在写入落地的
地方重新核对，而不是在写入之前先核对，因此第二个写入者无法插进读取与写入之间；`Ok(false)`
表示守卫已变、什么都没写入。`acts-store` 的每个后端（sqlite/postgres/sled）与内存
`MemoryStore` 都实现了它；未实现的自定义 `KvStore` 会直接得到拒绝，而不是静默退化为
「先检查再写入」。

`lease = false` 是嵌入式调用方或部署沿用旧约定的方式——此时
**「每个数据库一个写入者」需由你自行保证**：为每个服务器分配独立数据库，或在引擎之外协调。
通过 `EngineBuilder::set_store` 注入的嵌入式引擎同样不会获取租约：那就是你传入的原始
store，单写入者规则同样由你负责；`acts-server` 二进制才是替你管理租约的路径。

## 创建引擎

```rust
use acts::{Engine, Principal};

let engine = Engine::builder().start().await.unwrap();
// executor 代表一个调用者；直接驱动引擎的嵌入式调用者用的是引擎自身的身份
// （见"访问控制"一章）
let executor = engine.executor(&Principal::unrestricted());
```

## 部署和启动工作流

```rust
use acts::{Engine, Principal, Vars, Workflow};

let engine = Engine::builder().start().await.unwrap();

// 加载 YAML 模型
let model = r#"
id: my_model
name: my model
steps:
  - name: step 1
    uses: acts.transform.set
    params:
      a: 10
  - name: step 2
    uses: acts.app.javascript
    params: |
      return { data: a + 10 };
"#;
let workflow = Workflow::from_yml(model).unwrap();

// 部署模型
let executor = engine.executor(&Principal::unrestricted());
executor.model().deploy(&workflow).expect("fail to deploy workflow");

// 启动工作流
let mut vars = Vars::new();
vars.set("a", 0);
vars.set("pid", "w1");
executor.proc().start(&workflow.id, vars).expect("fail to start workflow");
```

## 关联项目

| 项目 | 说明 |
| ---- | ---- |
| [acts-server](https://github.com/yaojianpin/acts-server) | 基于 gRPC 的工作流服务 |
| [acts-channel](https://github.com/yaojianpin/acts-channel) | Rust 客户端库 |
| [acts-channel-py](https://github.com/yaojianpin/acts-channel-py) | Python 客户端库 |
| [acts-channel-go](https://github.com/yaojianpin/acts-channel-go) | Go 客户端库 |
