# 安装

`acts` 是一个快速、轻量、可扩展的工作流引擎库，使用 YAML 格式定义工作流并通过消息驱动架构执行和分发消息。

## 安装 acts 库

通过 `cargo` 命令安装：

```bash
cargo add acts
```

## 安装外部存储

支持以下外部存储：

```bash
# SQLite
cargo add acts --features store-sqlite

# PostgreSQL
cargo add acts --features store-postgres

# NATS
cargo add acts --features store-nats

# Redis
cargo add acts --features store-redis

# Sled
cargo add acts --features store-sled
```

## 创建引擎

```rust
use acts::Engine;

let engine = Engine::new().start().unwrap();
let executor = engine.executor();
```

## 部署和启动工作流

```rust
use acts::{Engine, Vars, Workflow};

let engine = Engine::new().start().unwrap();

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
