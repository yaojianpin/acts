# 存储分片与 RocksDB 后端设计（acts-store）

> **状态：已实现（2026-11-02）。** 后端 `RocksdbStore`、通用 `ShardedStore`、
> acts-server `[db]` 分片配置、分层背压与 `sys:status` 可观测性均已落地。
>
> **范围：** `crates/acts-store` 的 RocksDB 后端、pid 一致性哈希分片、顺序查询归并、
> 节点增删迁移，以及 `crates/acts` 的写路径背压和 `crates/acts-server` 的配置面。
>
> **非目标：** 不拆分单个 pid 的聚合家族；不提供跨物理库事务；不支持多分片 +
> 单行数据库租约；不把 model/package 等 global key 分散到多个分片。

---

## 1. 问题

引擎已经有两条并发轴：

1. scheduler 的 pid lane；
2. store-writer 的 pid 分片队列。

但一条 pid 的写入仍会落到**同一个物理数据库**。嵌入式后端（sled/RocksDB）每个库只有
一条写日志/一个 LSM 写管线，SQLite 只有一个写锁，Postgres 的写也最终受单库 WAL 限制。
当并发 pid 数增长时，瓶颈从「引擎队列」变成「数据库本身」。

分片的目标不是让一个 pid 变快，而是让**独立 pid 落在独立物理库**，同时保持：

- 一个进程的聚合、task/vars/message/delivery 行和派生索引仍在同一物理库；
- 该进程的 guarded batch 仍是单库原子事务；
- 跨分片的顺序查询仍能得到全局 key 序；
- 节点增删可以迁移，不要求停机重写全部数据；
- 数据库背压能逐层传到 acts 的写路径，而不是在数据库前形成第二个无界队列。

---

## 2. 物理模型

一个 `ShardedStore` 持有若干 `Shard`：

```text
ShardedStore
├── Shard A (Active)   -> SledStore / RocksdbStore / SqliteStore / PostgresStore / MemoryStore
├── Shard B (Active)   -> ...
└── Shard C (Draining) -> 只读迁移源
```

- `Shard::new(id, store)`：可写分片。
- `Shard::draining(id, store)`：不进入哈希环，但可读、参与扫描和点读回退；
  `rebalance()` 从它搬到 Active。
- `id` 是**稳定身份**，不是数组下标；默认使用物理路径/连接 URL。重排 shard 列表
  不改变 pid 的归属，重复 id 会被拒绝。
- key 布局**原样写入分片**：分片不添加 pid 前缀，也不改变 collection 层。
  因此已有数据库可以直接作为某个分片的物理库。

`ShardedStore` 实现完整的 `KvStore`，但扩展点为：

| 能力 | 行为 |
|---|---|
| `one` / `many` | 按 key 路由；未命中可跨分片回退 |
| `put` / `delete` | 单 key 路由（进程数据 key 依赖 id→pid 缓存或值内 `pid`） |
| `batch` | 只允许一个物理分片；跨分片直接报错 |
| `scan_prefix` | 全分片 fan-out，归并为全局 key 序 |
| `read_stable` / `read_view` | 单分片时委托原生快照；跨分片时重复采样至稳定 |
| `flush` | 并发 flush 所有分片 |
| `usage` | 汇总各分片 footprint；任一无法测量则报 unknown |
| `saturated` | 任一分片写闸门/底层后端饱和即为 true |

---

## 3. key 路由

### 3.1 路由分类

`classify_key` 返回三类之一：

```rust
enum KeyRoute {
    Pid(String),                              // key 直接带 pid
    Data { collection: String, id: String },  // 数据行/非 pid 索引，需 id→pid
    Global,                                   // 不属于任何进程
}
```

| 数据 | 典型 key | 分类 |
|---|---|---|
| 聚合 head / base / log | `kernel/aggregates/{hex(pid)}[/base|/log/…]` | `Pid`，hex 解码 |
| outbox/recovery/ready 索引 | `kernel/indexes/{outbox,recovery,ready}/{hex(pid)}` | `Pid` |
| recovery entry | `kernel/indexes/recovery-entries/{hex(pid)}.{wire}.{seq}` | `Pid` |
| `procs` | 数据 `procs-id-{pid}`；索引 `procs-{field}-{value}-{pid}` | `Pid`（id 就是 pid） |
| `tasks` | 数据 `tasks-id-{pid+wire}`；`tasks-pid-{pid}-{id}` | pid 索引直接 `Pid`；数据行 `Data` |
| `vars` | 数据 `vars-id-{pid+wire}`；`vars-pid-{pid}-{id}` | 同上 |
| `messages` | 数据 `messages-id-{msg_id}`；`messages-pid-{pid}-{id}` | 数据 `Data`；索引 `Pid` |
| `deliveries` | 数据 `deliveries-id-{delivery_id}`；`deliveries-pid-{pid}-…` | 同上 |
| `models` / `packages` / `events` / ACL / lease | `models-id-…`、`packages-id-…`、`events-…`、`acl_users-…`、`__acts_lease__` | `Global` |

### 3.2 索引值解码

collection 层的索引值经过 `encode_key_str`：只有 `[A-Za-z0-9]` 直通，其他字符编码为
`=XX` / `=XXXXXX`。`tasks-pid-{encoded(pid)}-{id}` 的 pid 必须先解码，才能和
`kernel/aggregates/{hex(pid)}`、`procs-id-{pid}` 指向同一个分片。

### 3.3 id→pid 学习

task/var/message/delivery 的**数据 key** 不直接包含可解析的 pid：

- task/var 的 id 是 `pid + wire`，没有分隔符；
- message/delivery 的 id 是随机 longid。

因此 `ShardedStore` 维护进程内缓存 `(collection, id) → pid`：

1. 写 batch 的 `Put` 值含 `"pid"` 字段时直接学习；
2. 同一 batch 的 `*-pid-{pid}-{id}` 索引键反推 pid；
3. 读到的数据行 JSON 含 `pid` 时学习；
4. 冷缓存点读未命中时，向所有分片查找一次并学习。

`batch_shard` 的判断顺序是：先学习 batch 内可见的 pid，再按 sibling index / cache
解析每个 key；全部 key 必须得到同一个分片，否则拒绝整笔 batch。

### 3.4 global key

所有 `Global` key 使用固定 token（`"\u{0}acts-global"`）走同一个环。这样：

- model + 其 trigger 行的 `deploy` batch 天然同分片；
- package、ACL、lease 与 model 行同分片；
- 不产生跨分片 batch；
- 代价是 model/package 写入集中在一个 global 分片。

### 3.5 跨分片 batch

物理库之间没有共享事务。`ShardedStore::batch` 在 key 跨分片时返回
`ActError::Store`，不会先写一部分。引擎正常的进程 batch（task/vars/message/delivery
的创建、更新、删除，`remove_proc_rows`，delivery 关闭）都由同一 pid 路由到同一分片；
model deploy 全是 global key，也在同一分片。

---

## 4. 一致性哈希环

### 4.1 1024 个虚拟节点

固定 `DEFAULT_VIRTUAL_NODES = 1024`，不暴露配置。环上的每个槽位有**固定 token**：

```text
slot_token(i) = XXH3("acts-slot\0" + i)          i ∈ [0, 1024)
owner(i)      = argmax_shard XXH3(shard_id + "\0" + i)
```

然后按 `slot_token` 排序，pid 的 hash 取第一个 `slot_token >= hash(pid)` 的槽位
（越过末尾回绕到 0）。

这不是「把 1024 个槽按 shard 数取模」。取模会在增删一个节点时改变所有槽位的归属：
例如 3 个节点时槽位 `i` 归 `i % 3`，4 个节点时归 `i % 4`，绝大多数槽位都会换主。
按槽位独立打分（最高随机权重）则：

- 增加节点只抢走它赢下的槽位，约 `1/(N+1)` 的 key 空间；
- 删除节点只释放它拥有的槽位；
- 其余槽位的 token 和 owner 都不变。

`crates/acts-store/tests/shard.rs` 的
`the_ring_has_1024_virtual_nodes_and_moves_a_minority_on_add` 用 10,000 个 pid
验证：从 3 分片加到 4 分片，移动比例在 5%–50% 之间，且 4 个分片都分到负载。
该测试同时验证重排 shard 列表不改变 pid 的归属。

### 4.2 稳定 id

环使用 `Shard::id` 而不是数组位置：

```rust
XXH3(shard_id, slot)  // shard_id 是路径/URL
```

因此：

- `[db].shards = ["b", "a"]` 与 `["a", "b"]` 的 pid 映射一致；
- 新增分片不需要给旧分片重新编号；
- 移动物理目录/改 URL 会被视为一个新分片（旧数据需要 rebalance / 保留 drain）。

### 4.3 负载边界

对 `N` 个 pid、`W` 个分片，期望每片 `N/W`；实际是二项分布。活跃 pid 数远小于分片数时
会有空分片；单个超热 pid 仍固定在一个分片（per-pid FIFO 与聚合原子性的代价）。
连续整数 pid（`1,2,3,...` 或零填充）经过 XXH3 后不会形成低比特聚集，`LaneRouter`
的同类测试也覆盖了这一点。

---

## 5. 顺序查询

### 5.1 scan_prefix 的全分片归并

`KvStore::scan_prefix` 的语义是「返回所有匹配 key」；`ShardedStore` 的做法是：

```text
for shard in active + draining:
    rows[shard] = shard.scan_prefix(key, options)
merge:
    sort (shard, key, value) by key ascending
    for each key group:
        choose the row from the ring owner if present, else the first copy
    if options.is_rev:
        reverse the whole merged vec
```

- 每个后端本身返回有序 key（Memory BTreeMap、sled、RocksDB、SQLite/Postgres
  `ORDER BY key`），分片层仍会重新排序，保证不依赖后端顺序。
- 反序是「升序归并后整体反转」，不是每个分片各自 `rev` 再拼接；后者无法得到全局序。
- 迁移期间同一个 key 可能出现在 owner 和旧分片；owner 副本优先。
- `scan_prefix` 当前对**所有**扫描都 fan-out 到 active + draining，即使 key 已经
  指向某个 pid。这是为了保证节点增删、rebalance 未完成时查询仍然完整；稳定运行后
  可以把单 pid 的 Eq/Range 优化为只查 owner + draining。

### 5.2 查询流水线

`KvCollection::query` 的路径不变：

1. `filter_id_set` 从索引扫描求全局 ID 集合；
2. `expr_ids` 使用 `scan_prefix` 的归并结果截取 id；
3. AND/OR 在 `HashSet` 上合并；AND 按基数排序求交；
4. `read_json_many` → `many` 按 key 分组，一次 `mget` 读一页/一批数据；
5. 排序/分页在 collection 层完成。

排序规则：

- 无 `order_by`：id 升序（稳定 tie-break）；
- 单个 `order_by` 且字段在 `ordered_index_fields`：`ordered_index_ids` 扫整个字段索引，
  按编码值分组到 `BTreeMap`，组内 id 排序；desc 反转组顺序；null/missing 按既定规则
  放首/尾；只读当前页的数据行；
- 其他情况：读全部匹配行，`cmp_order_docs` 全局排序，再 `skip/take`。

索引 key 的字典序**不等于**业务 `order_by` 序；collection 层会重新按编码值分组排序，
分片层只负责让索引扫描结果完整、去重、全局有序。

### 5.3 一致视图

- `read_stable`：所有 key 路由到同一分片时，直接使用该分片的原生快照；
  跨分片时按 `many` 重复采样，直到前后两次一致，超过 `STABLE_READ_ATTEMPTS` 报错。
- `read_view`：keys 和 prefix 都映射到同一分片时，委托该分片原生 `read_view`
  （正常聚合 family 路径）；global prefix 或跨分片时，keys + prefix 采样至稳定后合并。
- 跨分片查询不是跨库全局快照；单 pid 状态机由「同 pid 同分片 + 单分片 batch」保证。

---

## 6. RocksDB 后端

`RocksdbStore` 是嵌入式 LSM 后端，和 sled 一样是单进程文件锁。

| 能力 | 实现 |
|---|---|
| guarded batch | `TransactionDB` + `get_for_update(exclusive=true)`；guard 与 ops 同一悲观事务 |
| guardless batch | 单 op 直接 put/delete；多 op 一个 `WriteBatchWithTransaction<true>` |
| 快照读 | `db.snapshot()` + `ReadOptions::set_snapshot` |
| 前缀扫描 | `prefix_upper_bound` 生成独占上界；正序 `seek` / 反序 `seek_for_prev` |
| 持久性 | WAL 在写入返回前写入；`flush()` flush memtable + `flush_wal(true)` |
| footprint | `usage()` 递归统计数据库目录（内存后端为 unknown） |
| 背压 | `WriteGate` 限制 `RocksdbOptions::max_in_flight_writes`（默认 16） |

主要选项：

```rust
RocksdbOptions {
    max_in_flight_writes: 16,
    write_buffer_size: 64 MiB,
    max_write_buffer_number: 4,
    max_background_jobs: 0, // 0 = available_parallelism
    compression: true,     // Snappy
}
```

`TransactionDBOptions::default_lock_timeout(5s)`：持续变化的 guard 以超时/冲突返回，
由上层重试，而不是无限等待。

RocksDB 是 **GPLv2 OR Apache-2.0** 双许可；本仓库选择 Apache-2.0。第三方 attribution
见根目录 `THIRD_PARTY_NOTICES.md`，发布归档会同时携带 `LICENSE` 与该文件。

---

## 7. 背压与健康

三层依次传导：

```text
物理库写闸门满
  -> KvStore::saturated() == true
  -> StoreWriter::saturated() == backlog latch || db saturated
  -> 生产者停止接纳新工作；已接纳的写入在 DB gate 等待
  -> writer backlog 上涨并 latch，进一步阻止新增
```

- `ShardedStore::saturated()`：任一分片 gate 满，或任一底层 store 报 saturated。
  保守策略：一个分片满时整个引擎停止接纳，避免其他分片继续吸入后堆到同一个 writer
  backlog。
- `RocksdbStore::saturated()`：自己的写闸门。
- `SledStore` 目前没有后端级 saturated；使用 `ShardedStore` 时仍受分片 `WriteGate`
  保护。
- `sys:status.health.store_writer` 新增：

```json
{
  "state": "healthy",
  "store_writer": {
    "depth": 0,
    "high_watermark": 42,
    "saturated": false
  },
  "loops": [ ... ]
}
```

`state` 仍然只反映后台 loop 的**失败/退避**：背压是健康的上限，不是 degraded。
只有写入实际失败并连续达到 `DEGRADE_AFTER`，loop 才会进入 `degraded`。

---

## 8. 节点增删

### 8.1 增加分片

1. 新分片目录/URL 加入 `[db].shards`；
2. 读路径在 `search_on_miss = true`（默认）时立即能通过回退找到旧数据；
   迁移完成后可设为 `false`，让缺失 key 直接回答 absent，避免顺序扫全部分片；
3. 在维护窗口执行 `ShardedStore::rebalance()`（或 server 的
   `rebalance_on_start = true` 一次启动）；
4. rebalance 返回后再切换写流量。

### 8.2 移除分片

1. 把 URL 从 `shards` 移到 `draining_shards`（仍是 Active 之外的只读源）；
2. 设置 `rebalance_on_start = true` 或调用 `ShardedStore::rebalance()`；
3. rebalance 完成后清空 drain 列表，下一轮启动即不再打开旧库。

```toml
[db]
type = "rocksdb"                 # 或 "sled"
shards = ["/data/acts-0", "/data/acts-1"]
draining_shards = ["/data/acts-old"]
rebalance_on_start = true
lease = false                    # 多分片不能用单行租约
max_in_flight_writes = 16
```

### 8.3 rebalance 算法

```text
phase 1: 扫描全部 active + draining 的 key
         对每个 key:
           owner = route(key)
           若 owner 无副本: 复制到 owner
phase 2: 删除所有非 owner 副本
```

- **先复制后删除**：崩溃在中间只会留下重复副本，不会丢行；下一轮 rebalance 会清掉。
- 副本冲突时以 owner 副本为准，`conflicts` 计数；
- 迁移期间读优先 owner、未命中跨分片回退；
- 写流量应在 rebalance 完成后再切到新成员；否则 guard 在新 owner 上读不到旧行。

### 8.4 租约限制

lease 是单行 guard，检查必须和写入在同一个原子单元。跨物理库没有共享事务，
因此 acts-server 在多分片布局下拒绝 `lease = true`。多写者需要外部协调，或让每个
分片组只运行一个 server。

---

## 9. acts-server 配置

`DbConfig` 新增：

| 字段 | 含义 |
|---|---|
| `type = "rocksdb"` | 新后端 |
| `shards = [url, …]` | 物理分片；非空时替代 `database_url` |
| `draining_shards = [url, …]` | 只读迁移源 |
| `rebalance_on_start` | 打开时执行一次迁移 |
| `max_in_flight_writes` | 每分片/单 RocksDB 写闸门 |
| `search_on_miss` | 点读未命中时是否跨分片回退（默认 true） |

环槽位数固定为 1024，不暴露配置：修改它会重排大多数槽位，效果等同于一次全量
rebalance，因此只保留在 `DEFAULT_VIRTUAL_NODES` 常量里。

`open_store` 先校验分片组合，再打开每个物理库；`rebalance_on_start` 在 engine 启动前
完成。`ACTS_DATABASE_URL` 和 `database_url` 在 shards 非空时不参与。

---

## 10. 不变量与失败模式

| 不变量 | 保证 |
|---|---|
| 同一 pid 的数据/索引/聚合同分片 | `classify_key` + id 缓存 + sibling index；batch 计划验证 |
| 同一 batch 单分片原子 | 跨分片 `batch` 直接报错 |
| 顺序扫描全局 key 序 | 全分片 fan-out + 归并 + owner 去重 |
| 反序等于正序反转 | 升序归并后整体反转 |
| rebalance 不丢数据 | copy 先于 delete；冲突保留 owner |
| drain 分片可读 | 加入扫描和点读回退 |
| 背压可传导 | `KvStore::saturated` + `StoreWriter` latch + `sys:status` |
| 单 pid 不拆分 | 一致性哈希只按 pid 路由 |

已知边界：

- 单个热 pid：无法拆分；
- global key（model/package/event/ACL）集中在一个分片；
- 冷缓存的 task/var/message/delivery 数据 key 第一次点读会 fan-out；
- 跨分片查询不是全局快照；
- `scan_prefix` 尚未按单 pid 路由优化；
- 多分片与 lease 互斥；
- 没有跨分片事务/2PC。

---

## 11. 验证

| 测试 | 覆盖 |
|---|---|
| `crates/acts-store/tests/shard.rs` | 1024 槽、增删移动比例、重排稳定、进程 batch 同分片、全局有序/反序、跨分片 batch 拒绝、guarded batch、rebalance/draining、sled 分片、写闸门饱和 |
| `crates/acts-store/tests/rocksdb.rs` | 跨后端 store/kv/guarded 套件、Eq/Range/反序、文件持久化、stale guard |
| `crates/acts-store/src/lib.rs` snapshot tests | RocksDB 并发写者下的单版本 view |
| `crates/acts/src/utils/lane.rs` | 连续整数 pid 在 lane 上的分布 |
| `crates/acts/src/store/writebehind.rs` | 后端 pressure 进入 writer saturated |
| `crates/acts/src/export/actions.rs` | `sys:status.health.store_writer` |
| `crates/acts-cli/src/cmd/status.rs` | backpressure 行渲染 |
| `crates/acts-server/src/lib.rs` | `[db].shards` 解析、lease 拒绝、draining/rebalance 校验、sharded sled 打开 |

`examples/stress` 的 CI 矩阵只跑 100,000 工作流 baseline：`sled`、`rocksdb` 各 4 个物理分片、4 个 scheduler worker（write-behind 复用该并发），与 4 vCPU runner 匹配；分片再多只会放大全分片扫描与后台线程，不能增加吞吐。`RocksdbStore` 还需要 bindgen 的 `libclang`（C++ 源码由 `cc`
编译，不需要 cmake）。

---

## 12. 后续可做

1. 单 pid 的 Eq/Range 扫描只查 owner + draining，而不是全分片；
2. rebalance 的在线限速/断点续跑与独立 CLI；
3. model/package 按 mid 分片（需要 model + trigger 的批量路由一起改）；
4. 每分片健康/队列指标，而不是只汇总 `store_writer`；
5. 基于 2PC 或全局事务日志的跨分片 batch（当前明确拒绝）。
