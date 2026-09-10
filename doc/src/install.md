# 安装

`acts` 是一个快速、轻量、可扩展的工作流引擎库，使用 YAML 格式定义工作流并通过消息驱动架构执行和分发消息。

## 安装 acts 库

通过 `cargo` 命令安装：

```bash
cargo add acts
```

## 安装外部存储

外部存储后端（sqlite/postgres/redis/nats/sled）在独立的 `acts-store` crate 中，
启用对应 feature 后从 `acts_store` 导入后端，再通过 `EngineBuilder::set_store`
指定（不设置时默认使用内存存储 `MemoryStore`）：

```bash
# SQLite
cargo add acts-store --features sqlite

# PostgreSQL
cargo add acts-store --features postgres

# NATS
cargo add acts-store --features nats

# Redis
cargo add acts-store --features redis

# Sled
cargo add acts-store --features sled
```

```rust
use acts::Engine;
use acts_store::SqliteStore; // 或 PostgresStore / RedisStore / NatsStore / SledStore
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

## 创建引擎

```rust
use acts::Engine;

let engine = Engine::builder().start().await.unwrap();
let executor = engine.executor();
```

## 部署和启动工作流

```rust
use acts::{Engine, Vars, Workflow};

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
    uses: acts.transform.code
    params: |
      return { data: a + 10 };
"#;
let workflow = Workflow::from_yml(model).unwrap();

// 部署模型
let executor = engine.executor();
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
