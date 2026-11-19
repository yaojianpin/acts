# 表达式引擎（acts-expr）资源合同与执行设计

- 状态：已实现
- 日期：2026-11-19
- 范围：`crates/acts-expr`、`crates/acts/src/env`、
  `crates/acts/src/scheduler/shell/facts.rs`、`scheduler/shell/caps/assign`

## 1. 背景

表达式不是内部配置，而是工作流作者可控制、会在调度热路径反复执行的数据：

- 分支守卫、步骤规则（`rules[].if`）、`assign.owner`；
- Task/Package 参数与输出模板；
- Query Filter、候选人表达式、消息投影。

当前语法已经覆盖工作流所需的字面量、算术、比较、布尔短路、成员/索引访问、
函数与内置方法；问题不在语法功能，而在**资源合同**：任意合法表达式必须被限制
在可预测的 CPU、内存、调用栈、输出、容器遍历和宿主调用边界内，不能因为
“语法合法”而让一个 Scheduler Worker 无限运行，或让一个进程 abort。

本设计把 `acts-expr` 从“可以执行的用户数据”定义为一个有明确预算、类型和错误
合同的受控程序。

### 1.1 目标

1. 编译期有界：Source、Token、Node、逻辑 AST 深度、Literal、Identifier、
   调用参数、常量 Regex 各有硬上限。
2. 求值不依赖用户 AST 的宿主调用栈：使用显式 Frame/Value 栈，而不是递归
   `eval_node`。
3. 每次求值有 Fuel、Deadline、Cancellation、Allocation、Output、
   Collection Items 和 Function Calls 预算。
4. 上下文 Value、函数返回值、最终结果都有统一的 `ValueLimits`。
5. 宿主函数有 `FunctionSpec`：纯度、成本、最大调用次数、最大输出；Panic 被
   隔离为结构化错误。
6. 常量 Regex 在编译期预编译；动态 Regex 使用有界 LRU；Pattern/Input 和错误
   回显有上限。
7. 支持类型化结果、带版本/函数代际的编译缓存、稳定错误码和有界错误文本。

### 1.2 非目标

- 不把表达式变成通用脚本语言：仍然没有赋值、语句、三目、集合字面量、
  `in`、推导式或用户自定义语法。
- 不在表达式内执行网络/磁盘 I/O；需要外部数据时在进入表达式前准备 Context，
  或通过 Effect/Package 协议执行。
- 不提供可抢占的同步函数。`FunctionSpec` 能拒绝已声明 effectful 的调用、
  限制调用次数和输出，但不能中断一个正在阻塞的宿主函数。
- 不在本 crate 内实现 Prometheus 指标；`Error::code()`、`CacheStats` 和
  `Expr` 元数据是上层指标的数据源。

## 2. 总体结构

```text
Source
  → 字节上限
  → Lexer：Token 数、Identifier 字节、String Literal 字节、非有限浮点拒绝
  → Parser：Node 数、逻辑 AST 高度、调用参数数、常量字符串折叠
  → 静态类型检查（ExprType）
  → 常量 Regex 预编译
  → Expr {
        nodes: Arc<[Node]>, root, depth,
        estimated_cost, source_hash,
        expected_type, profile, regex_pool
    }

Expr + Context + EvalBudget
  → 显式 Frame/Value 栈执行
  → 每一步计 Fuel；分配/输出/遍历/函数调用计费
  → 周期性检查 Deadline/Cancellation
  → 类型化结果校验 + ValueLimits + 最终输出预算
```

`Expr` 是不可变的，节点使用 `Arc<[Node]>`，可跨线程共享；`Expr::clone` 只克隆
句柄。一次编译可以绑定不同的 `BudgetProfile`，缓存命中不会重新解析。

## 3. 编译合同

### 3.1 `CompileLimits`

```rust
pub struct CompileLimits {
    pub source_bytes: usize,
    pub tokens: usize,
    pub nodes: usize,
    pub depth: usize,
    pub literal_bytes: usize,
    pub identifier_bytes: usize,
    pub call_args: usize,
    pub regex_pattern_bytes: usize,
}
```

默认值（Task Profile）：

| 项 | 默认值 | 约束点 |
|---|---:|---|
| `source_bytes` | 64 KiB | `Expr::build` / `Parser::new` |
| `tokens` | 8192 | Lexer 每产生一个真实 Token |
| `nodes` | 8192 | Parser Arena `push` |
| `depth` | 128 | Parser 递归嵌套 + 逻辑 AST 高度 |
| `literal_bytes` | 16 KiB | 单个解码后的字符串字面量 |
| `identifier_bytes` | 256 B | Identifier / Member 名 |
| `call_args` | 256 | 单次调用参数 |
| `regex_pattern_bytes` | 8 KiB | 常量 Regex Pattern |

### 3.2 逻辑 AST 深度

只限制括号、Unary、Call 的 Parser 递归不够：左结合二元链

```text
1 + 1 + 1 + ...
```

会形成深度接近操作数数量的左深树。Parser 在每次 `push(Node)` 时计算

```text
height(literal) = 1
height(Binary(op, l, r)) = 1 + max(height(l), height(r))
height(Member(t, _)) = 1 + height(t)
height(Call(args)) = 1 + max(height(args))
```

超过 `CompileLimits::depth` 立即返回 `Error::TooDeep`。Parser 自身递归深度
同样使用该上限，因此括号、Unary、嵌套 Call 也不会先把宿主栈打穿。

### 3.3 常量折叠

`'a' + 'b' + ...` 在 Parser 中折叠为一个 `Node::Str`：

- 只在连接结果不超过 `literal_bytes` 时折叠；
- 累计折叠字节不超过 `source_bytes`，避免编译期 O(n²) 复制；
- 超过预算时保留 `Binary(Add)`，由运行时 allocation 预算约束；
- 折叠减少运行时重复复制，但不替代运行期额度。

### 3.4 常量 Regex

`is_match('literal')` 的 Pattern 在 `Expr::compile` 时预编译并存入
`RegexPool::constants`：

- 同一表达式执行 10 万次只编译一次；
- 非法常量 Pattern 在编译/模型部署阶段失败；
- Pattern 长度超过 `regex_pattern_bytes` 在编译期拒绝；
- 动态 Pattern 见第 7 节。

## 4. 运行预算：`EvalBudget`

```rust
pub struct EvalLimits {
    pub steps: u64,
    pub allocation_bytes: usize,
    pub output_bytes: usize,
    pub function_calls: u32,
    pub collection_items: u64,
    pub deadline: Duration,
    pub value: ValueLimits,
    pub allow_effectful_functions: bool,
}

pub struct EvalBudget { /* remaining counters + deadline + cancellation */ }
```

`EvalBudget::for_profile(profile)` 创建一次求值的预算；带 `_with_budget` 的入口
接受调用方持有的预算，预算是**消耗式**的，不是每次重置——Query Filter 可以
用一个预算覆盖整个查询，而不是每行重新获得完整额度。

### 4.1 计费规则

| 操作 | 计费 |
|---|---|
| 每个 Frame/Node | 1 Step |
| 原生函数调用 | 1 Function Call + `FunctionSpec::base_cost` Step |
| List/Map 比较、`contains` | 每候选 1 Collection Item + 1 Step |
| 字符串拼接 | 新长度计入 Allocation |
| 函数参数数组 | `len * size_of::<Value>()` 计入 Allocation |
| 函数返回值 | 计入 Allocation；受 `FunctionSpec::max_output_bytes` 限制 |
| 常量 Regex 编译 | 编译期，不进运行预算 |
| 动态 Regex 编译 | `32 + pattern_len / 8` Step |
| Regex 匹配 | `1 + input_len / 64` Step |
| 字符串方法 | 文本长度 / 64 Step（有下限 1） |
| 最终结果 | 结构检查后计入 Output |

Step 每 64 次检查一次 Deadline 和 Cancellation；函数返回后、最终结果返回前
再检查一次。容器遍历通过 `charge_items` 同时消耗 Collection Items 和 Steps，
因此长比较不会绕过 Deadline。

### 4.2 预算错误

预算耗尽必须返回独立错误，不能转成 `false`：

```rust
Error::BudgetExceeded {
    kind: BudgetKind::Steps
        | BudgetKind::Allocation
        | BudgetKind::Output
        | BudgetKind::FunctionCalls
        | BudgetKind::CollectionItems,
    limit,
}
Error::DeadlineExceeded
Error::Cancelled
```

`Error::code()` 对预算错误统一为 `expression_budget_exceeded`，具体维度由
`BudgetKind` 区分；超时和取消分别是 `expression_deadline_exceeded` 和
`expression_cancelled`。上层可以把它们映射为 Task Failure、规则
`validation_failed` 或 Query Error，但不能静默走错分支。

## 5. `ValueLimits` 与值边界

Source 预算不约束 Context：Process Vars、Snapshot Data、Message Params 和
Function 返回值可以携带巨大或深层嵌套的 List/Map。

`ValueLimits` 默认值：

| 项 | 默认值 |
|---|---:|
| `depth` | 64 |
| `nodes` | 100,000 |
| `list_items` | 100,000 |
| `map_entries` | 100,000 |
| `key_bytes` | 1 KiB |
| `string_bytes` | 1 MiB |
| `total_bytes` | 64 MiB |

检查点：

1. 求值前对 Context 的注入值做一次迭代扫描；
2. 每个原生函数结果检查（并受 `FunctionSpec::max_output_bytes` 额外约束）；
3. 最终结果检查；
4. `Value::from_json_checked` 在转换前检查 JSON 树；
5. `Value::to_json_checked` 在转换前检查值树。

值遍历是显式的 `Vec<(value, depth)>` 栈，不递归。非有限 Float（NaN/Infinity）
在 Context、函数结果和最终结果处拒绝；JSON 转换保留 `Convert` 错误语义。

## 6. 求值器

### 6.1 显式栈

求值器不递归访问 AST。`Frame` 描述“下一步做什么”，`Value` 栈保存已完成的值：

```text
Frame::Eval(node)
Frame::Not / Neg / Binary / CheckBool
Frame::ShortCircuit(op, right)
Frame::Member(name) / Index
Frame::Call { name, argc }
Frame::Method { name, argc }
```

`&&`/`||` 先求左侧；短路成立时直接压入 Bool，否则压入
`CheckBool(op)` 后再求右侧，确保右侧也必须是 Bool。非短路二元运算先求左、
再求右；Call/Method 的参数按源代码顺序求值。AST 深度只影响 Frame 数量，
不会转化为 Rust 调用栈深度。

### 6.2 类型与错误

现有语义保持不变：

- 两个 int 保持 int，溢出是错误；uint/int 混合用 128 位精确计算；
- 任一侧 float 结果为 float，非有限结果拒绝；
- `+` 是唯一的字符串算术；
- 比较/相等跨 int/uint/float，List/Map 结构相等，不同种类不相等；
- 类型不匹配是 `Error::Type`，不隐式 truthy/截断。

## 7. Regex

### 7.1 常量 Pattern

编译期扫描 `Node::Method(_, "is_match", [Node::Str(pattern)])`，对合法 Pattern
预编译到 `RegexPool::constants`。非法 Pattern 在编译期返回 `Error::Function`，
文本包含截断预览与哈希，不包含完整 Pattern。

### 7.2 动态 Pattern

动态 Pattern 进入每个 `Expr` 自己的有界 LRU：

| 项 | 上限 |
|---|---:|
| Entries | 128 |
| Estimated Bytes | 16 MiB |
| Pattern | `CompileLimits::regex_pattern_bytes`（默认 8 KiB） |
| Input | 1 MiB |

- Key 为 Pattern 原文，淘汰最久未使用项；
- 单个 Regex 估算大小超过整个 Cache 时不缓存，但仍然返回给本次调用；
- 编译和匹配都计 Step；
- 错误只输出 `pattern.len()`、FNV 哈希、最多 128 字符的转义 Preview 和
  "is not a regular expression"。

Rust `regex` 本身避免传统回溯引擎的指数级 ReDoS；这里解决的是重复编译、
Pattern/Input 长度、缓存内存和错误回显。

## 8. 宿主函数合同

```rust
pub struct FunctionSpec {
    pub purity: FunctionPurity, // Pure / ContextRead / Effectful
    pub base_cost: u64,
    pub max_calls: u32,
    pub max_output_bytes: usize,
    pub deterministic: bool,
    pub thread_safe: bool,
}
```

默认：`ContextRead`、1 Step、1000 次调用、1 MiB 输出。

- `Context::set_spec(name, spec)` 绑定合同；未声明的函数使用默认 Spec。
- `Effectful` 默认被 Profile 拒绝；调用方必须通过
  `EvalBudget::set_allow_effectful_functions(true)` 显式放行。
- 每次调用检查全局 `function_calls`、每个名字的 `max_calls`、
  `base_cost`、参数分配和输出大小。
- 调用点使用 `catch_unwind(AssertUnwindSafe(...))`，Panic 转为
  `Error::FunctionPanicked`，Worker 存活；这不能捕获 Stack Overflow/Abort。
- 同步函数不能被抢占：函数不得执行阻塞 I/O；需要异步数据的函数必须在进入
  表达式前准备 Context，或通过 Effect 协议执行。

## 9. 类型化结果

```rust
pub enum ExprType {
    Any,
    Bool,
    Int,
    Number,
    String { max_bytes: usize },
    Duration { max_ms: u64 },
}
```

- 编译时能静态确定的类型（字面量、比较/布尔运算、可推导算术）立即检查；
  类型不符在编译期返回 `Error::ResultType`。
- 编译期未知的类型在运行时检查，错误同样稳定。
- `String { max_bytes }` 检查最终字符串长度；`Duration { max_ms }` 要求
  `0..=max_ms` 的整数毫秒。
- Gateway 条件可编译为 `Bool`；Owner/Assignee 可编译为受限 String；Timeout/
  Retry 参数可编译为 Duration。

## 10. 错误合同

`Error` 的新增分类：

```text
TooLarge / TooManyTokens / TooManyNodes / TooDeep / TooManyArguments
BudgetExceeded { kind, limit } / DeadlineExceeded / Cancelled
ValueLimit { what, limit }
FunctionPanicked / FunctionRejected
ResultType { expected, got }
NonFinite { operation }
```

规则：

1. `Error::code()` 是稳定的机器码，调用方不得解析 Display 文本。
2. `Display`/`Debug` 最多输出 1 KiB；Source、Pattern、Value、宿主消息不得
   全文进入错误、日志或 Transport。
3. `Error::position()` 只对 Parse 错误返回字节 Offset；Unicode 不切在
   UTF-8 中间。
4. 上层若持久化表达式失败，记录应只保存 `code`、`expression_id`、
   `source_hash`、`position`、`model_version`、`node_id`、`retryable` 等摘要；
   通过 Model Version + Source Hash 定位原表达式。
5. 上层若允许 Catch 表达式错误，Catch 应只接收安全结构（`code`、
   `position`、`retryable`），不接收原 Source 或完整内部错误链。

## 11. 编译缓存

```rust
pub struct CacheKey {
    pub source_hash: u64,
    pub semantics_version: u32,
    pub function_generation: u64,
    pub expected_type: ExprType,
    pub profile: BudgetProfile,
    pub limits: CompileLimits,
}
```

`ExprCache`：

- 默认最多 1024 项、4 MiB；
- 按 LRU 淘汰；单条超过整个 Cache 时不缓存；
- Entry 保存 Source 副本并逐字比较，Hash 冲突不会返回错误程序；
- `CacheStats { entries, bytes, hits, misses, evictions }`；
- `get_or_compile` 在锁外编译，避免长编译持锁。

缓存必须由调用方按 Model Version、Function Registry Generation、Profile 和
Limits 建键。旧 Compiled Artifact 可以继续服务绑定旧 Model Version 的流程，
不应全局热替换改变运行中流程语义。

## 12. 预算 Profile

### 12.1 编译上限

| Profile | Source | Tokens | Nodes | Depth | Literal | Ident | Args | Regex |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Gateway | 16 KiB | 4096 | 2048 | 128 | 8 KiB | 256 B | 64 | 4 KiB |
| Task | 64 KiB | 8192 | 8192 | 128 | 16 KiB | 256 B | 256 | 8 KiB |
| Query | 8 KiB | 2048 | 1024 | 128 | 4 KiB | 256 B | 32 | 2 KiB |
| Assignment | 16 KiB | 4096 | 2048 | 128 | 8 KiB | 256 B | 64 | 2 KiB |

### 12.2 运行预算

| Profile | Steps | Allocation | Output | Calls | Items | Deadline |
|---|---:|---:|---:|---:|---:|---:|
| Gateway | 20,000 | 1 MiB | 1 KiB | 64 | 10,000 | 10 ms |
| Task | 100,000 | 8 MiB | 1 MiB | 1000 | 100,000 | 50 ms |
| Query | 10,000 | 512 KiB | 1 KiB | 32 | 10,000 | 10 ms |
| Assignment | 20,000 | 1 MiB | 256 B | 64 | 10,000 | 10 ms |

Gateway/Query/Assignment 另把 Value Nodes、List Items、Map Entries 收紧到
10,000，String 收紧到 256 KiB/64 KiB；其余沿用 `ValueLimits::default()`。

当前引擎的 `Expr::compile`、分支守卫和规则条件使用 Task Profile；
`compile_with_profile` 为 Gateway/Query/Assignment 调用方提供显式入口。
Query 的总预算应由调用方复用同一个 `EvalBudget`，而不是每行重置。

## 13. 调用方与集成

### 13.1 作用域注入

`crates/acts/src/env` 只注入表达式 `references()` 返回的名字：

```text
sealed $name → 内置 → 插件函数 → step data → user var → task var
```

未解析的名字保持未绑定，读取时是 `Unknown`，不会被悄悄当成 `null`。

### 13.2 分支守卫与规则

`facts::evaluate_predicate`：

- 编译失败 → `PredicateValue::Invalid`，不是 `False`；
- 未知变量 → `Invalid`；
- 非 Bool/Null 结果 → `Invalid`；
- 求值错误（包括预算、Value、函数错误）→ `Invalid`；
- 短路和预算语义沿用 `acts-expr`。

`Step.rules` 的条件走同一条 seam，因此规则条件也受同一编译和运行预算约束。

### 13.3 参数/输出模板

`Environment::eval`：

1. `Expr::compile(expr)`（Task Profile）；
2. 只注入 referenced names；
3. `program.eval(&ctx)`；
4. `Value::to_json()` 在默认 `ValueLimits` 下转换；
5. 反序列化为调用方类型。

`${{ }}` 填充、`fill_params`、`fill_outputs` 的错误传播保持原语义：无法求值时
按调用方的既有约定处理（例如输出填充回退为 `null`），但预算/Value/类型错误
不会被伪装成普通 `false`。

## 14. 兼容与迁移

### 14.1 Semantics Version

`SEMANTICS_VERSION = 1`。Parser/Evaluator 改变结合性、类型或方法行为时递增，
并进入 `CacheKey`；旧 Artifact 不能在新语义下被错误复用。

### 14.2 行为变化

升级会引入以下可观察变化：

- 超过 Source/Token/Node/Depth/Literal/Identifier/Args/Regex 上限的表达式在
  编译期失败，而不是运行到深处或 Stack Overflow；
- 非法常量 Regex 在编译期失败；
- 超过 `ValueLimits` 或含 NaN/Infinity 的 Context/函数结果/最终结果失败；
- 预算耗尽返回结构化错误，不再长时间占用 Worker，也不转成 `false`；
- 超过 `FunctionSpec::max_calls`/`max_output_bytes` 的宿主函数调用失败。

### 14.3 迁移步骤

1. **离线扫描**：用新 `CompileLimits` 编译全部 Model 表达式，输出
   `accepted` / `source_too_large` / `too_many_tokens` / `too_many_nodes` /
   `too_deep` / `too_many_arguments` / `regex_invalid` / `type_error` /
   `estimated_cost_high`。
2. **Observe-only**：先记录将被拒绝的表达式和 Value，不阻断；确认业务基线。
3. **Enforce**：设置保守硬限并切换；Stack Overflow 类问题不应长期
   Observe-only。
4. **回滚**：新 Artifact 记录 Semantics Version；旧二进制无法解释新语义时，
   部署需携带最低兼容版本和 Artifact 回滚策略。

## 15. 测试与覆盖率

`crates/acts-expr` 的测试按合同分层：

- 编译边界：每个 `CompileLimits` 字段独立错误；Binary 链、折叠预算；
- 运行预算：Step/Allocation/Output/Items/Calls/Deadline/Cancellation；
- Value：深度/Nodes/Items/Key/String/Total、非有限值、迭代 JSON/比较/格式化；
- 求值器：显式栈、短路、每个 Binary/Unary/Member/Index/Call/Method 形状、
  容器的每类相等分支；
- 函数：`FunctionSpec`、调用次数、输出大小、effectful 拒绝、Panic 隔离；
- Regex：常量预编译、动态 LRU、Pattern/Input 上限、错误 Preview；
- 类型与缓存：全部 `ExprType` 分支、Cache 命中/淘汰/清理/超限/坏表达式。

当前结果：

| 指标 | `--all-features` | 无 features |
|---|---:|---:|
| Line Coverage | 98.42% | 98.34% |
| Function Coverage | 95.16% | 95.91% |
| Region Coverage | 97.47% | 97.37% |

验证命令：

```sh
cargo test -p acts-expr --lib --all-features
cargo test -p acts-expr --lib
cargo clippy -p acts-expr --all-targets --all-features -- -D warnings
cargo clippy -p acts-expr --all-targets -- -D warnings
cargo fmt --all --check
cargo llvm-cov -p acts-expr --lib --all-features --summary-only
cargo test -p acts --lib
```

## 16. 复杂度与风险

- **同步宿主函数不可抢占**：`FunctionSpec` 只能事后限制输出/次数，不能中断
  阻塞 I/O。生产注入必须遵守同步、短时、无外部副作用的合同；Effectful 默认
  拒绝。
- **Context 校验成本**：每次求值扫描注入值；引擎只注入 `references()`，因此
  通常是少量数据。超大 Context 会在执行前被 Value 限额拒绝。
- **Regex Cache 是按 Expr 的**：大量表达式各自持有动态 Cache 会放大内存；
  常量 Pattern 走预编译，动态 Pattern 应在调用方控制表达式数量。
- **Cache Generation 由调用方负责**：函数注册变化必须更换
  `function_generation`，否则旧 Artifact 会继续使用旧绑定。
- **常量折叠有编译期成本**：累计折叠字节以 `source_bytes` 为上限，避免长常量
  链在编译期 O(n²) 复制。
- **错误可诊断性**：完整 Source/Pattern/Value 不进错误；持久化定位依赖
  `source_hash` + Model Version，需要上层提供可访问的诊断记录。

## 17. 结论

`acts-expr` 仍然保持小语言、单次编译、Arena AST 和可跨线程共享的优点，但每个
表达式现在是有资源合同的受控程序：

```text
多维编译预算
→ 显式栈求值
→ Fuel/Allocation/Output/Items/Deadline/Cancellation
→ Value 入口预算
→ Regex 常量池与有界缓存
→ FunctionSpec 与 Panic 隔离
→ 类型化 Artifact + Semantics Version + 有界错误 + 缓存代际
```

最严重的历史问题——合法超宽 Binary 链绕过 `MAX_DEPTH`、递归求值导致
Stack Overflow/进程 Abort——已从结构和预算两层消除；其余放大路径也被编译、
运行、Value、Regex、函数和缓存合同覆盖。
