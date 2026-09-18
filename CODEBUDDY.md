# CODEBUDDY.md This file provides guidance to CodeBuddy when working with code in this repository.

## 仓库定位

这是个人的 **Rust 学习工作区**，不是单一产品。根目录下**没有** `Cargo.toml`，四个子项目彼此独立，必须 `cd` 进各自目录才能跑 cargo 命令。

| 目录 | 性质 | 说明 |
|---|---|---|
| `LeetCode/` | 刷题（Rust + C++ 双语） | 每日一题 + 17 个算法专题文档 |
| `http_server_rust/` | 学习项目（进行中） | C++ HTTP Server 用 Rust 重写，手写 HTTP/1.1 |
| `roze/` | 大型 Rust 微服务框架 | 63 个 crate 的 workspace，代码生成器 `rozectl` |
| `thread_pool/` | 空壳 | edition 2024，`src/main.rs` 仅 45 字节，无依赖 |

子项目各自已有规则文件，冲突时**以更具体的那份为准**：`LeetCode/CLAUDE.md`、`http_server_rust/CLAUDE.md`（比该目录下的 `CODEBUDDY.md` 新，后者写于多数模块还是占位符时）、`roze/README.md` 与 `roze/docs/`。

## 常用命令

### LeetCode（Rust 侧）

```bash
cd LeetCode
cargo test                      # 跑全部题的单元测试
cargo test solution_7_23        # 只跑某一天（测试名含日期）
cargo run -p leet_code_daily    # 跑统一入口 main.rs
```

Rust workspace 成员为 `lc_test`、`leet_code_daily`。新题文件需在 `leet_code_daily/src/main.rs` 里手动 `#[path] mod` 登记，否则不会被编译。

### LeetCode（C++ 侧）

```bash
cmake -B LeetCode/leet_code_daily_cpp/build -S LeetCode/leet_code_daily_cpp  # 新增 .cpp 后需重跑
cmake --build LeetCode/leet_code_daily_cpp/build --target day_7_23           # 只编译某天
ctest --test-dir LeetCode/leet_code_daily_cpp/build --output-on-failure      # 跑全部
g++ -std=c++20 -O2 -o /tmp/7_23 LeetCode/leet_code_daily_cpp/src/7_23.cpp && /tmp/7_23  # 单文件快验
```

C++ 无需登记：`CMakeLists.txt` 自动为 `src/` 下每个 `.cpp` 生成目标名 `day_{月}_{日}`。

### http_server_rust

```bash
cd http_server_rust
cargo check                     # 开发期优先用它，快速反馈
cargo test --test request_test  # 跑 tests/ 下的单个集成测试文件
RUST_LOG=debug cargo run        # 带 tracing 日志启动（127.0.0.1:8080）
cargo clippy && cargo fmt       # lint + 格式化
```

### roze

```bash
cd roze
cargo build -p roze-core                                  # 单 crate 构建（workspace 很大，别无参全量）
cargo test -p rozectl -- --skip postgres --skip mysql     # rozectl 生成器测试（无需外部 DB）
bash scripts/production-smoke.sh                          # 生产冒烟：生成代码编译测试 + 核心 crate 测试
bash scripts/production-smoke.sh --with-compose           # 额外拉起 Etcd/Kafka/Redis/Pg/MySQL/ES 等集成依赖
```

`rozectl` 相关测试分三片（SQLite/parser、Postgres、MySQL），后两片需 `ROZECTL_TEST_POSTGRES_URL` / `ROZECTL_TEST_MYSQL_URL` 环境变量，CI 分开跑。

## 架构

### LeetCode —— 双语刷题 + 教学文档

每天一道题写两遍：Rust（edition 2024，纯标准库）和 C++（C++20，只用标准库 + 项目内 `test_util.h`）。算法思路一致，用来对比两门语言表达差异。

- 目录：`leet_code_daily/`（Rust，文件命名 `{月}_{日}.rs`）、`leet_code_daily_cpp/`（C++，`{月}_{日}.cpp`）、`Docs/`（17 个专题，从复杂度分析到数学，每个含讲解 + Rust 模板 + 例题 + 训练题）、`training/`（散题，目前 1 个文件）。
- 每个解法文件结构是**强制**的：文件头注释（题目描述 / 算法推导 / 复杂度）→ 顶层实现函数 → `Solution::xxx` 仅做转发（保证既能提交 LeetCode 又能独立测试）→ `///` doc comment → 函数体内用 `// ---- 步骤 N：xxx ----` 分段 → 末尾自带测试。用了不常见语法（`entry` API、`wrapping_neg`、结构化绑定等）必须加注释解释，标准是"一年后回看还能秒懂"。
- Rust 侧测试写在文件末尾 `#[cfg(test)] mod tests`；C++ 侧写在 `main()` 里，用 `test_util.h` 的 `CHECK_EQ`，最后 `return test_util::summary("{月}_{日}")`，有失败返回非零让 ctest 判失败。用例统一分三类：`示例 N`（官方示例）、`边界：xxx`、`规模烟雾：xxx`（n=10⁵，验证不超时）。

**刷题时的教学方式（来自 `LeetCode/CLAUDE.md`，最高优先级）**：苏格拉底式提问，不直接给答案、不直接说"这是 DP/二分/贪心"。流程是：理解题意 → 分析暴力解 → 找突破口 → 逐步缩小范围 → 让我自己说出算法 → 设计状态 → 复杂度分析 → 代码 → 总结套路。提示分五级，Level 1 只提问，Level 5 才给完整思路，**未经允许不得跳级**。方向错了不要直接纠正，用"这个思路最终复杂度是多少？""数据范围允许吗？"引导自己发现。解完后不要立刻结束：分析核心突破点、卡住的环节、思维漏洞，推荐 3 道难度递进的同类型题，并让我自己归纳模板。

由 AI 补全测试用例、并在每题完成后把套路追加到 `Docs/18_每日一题模板手册.md`（仅 Rust 模板），追加时同步更新总览表、跨套路速查表、自检清单和更新日志。**测试失败时的修复由用户自己完成。**

### http_server_rust —— C++ 服务器 Rust 重写

把 C++ 的 Main/Sub Reactor + epoll + 线程池模型用 Rust（tokio）重写，属学习性质，**已完成度约 30%**（`CODEBUDDY.md` 写作时多数模块还是 1 行占位符，`CLAUDE.md` 反映当前真实状态）。

四层架构，依赖严格自上而下，无环：

```
main.rs / lib.rs    入口 + 公开 API 再导出
src/server/         第 3 层：TcpListener + accept 循环 + 每连接 tokio::spawn
src/http/           第 2 层：手写 HTTP/1.1（解析、响应构造、路由、连接状态机）
src/infra/          第 1 层：ServerError 枚举、裸 Socket 的 RAII 封装
tokio / axum        第 0 层：异步运行时 + axum 的 StatusCode / IntoResponse
```

要点：

- **README.md 是设计文档与权威参考实现**，每个 TODO 模块的完整代码都以代码块形式存在于其中；实现某个模块前先读 README，抄进源文件后要解开 `mod.rs` 里的 `pub struct` 注释和 `lib.rs` 里的 `pub use` 注释。
- 走的是"依赖用 axum 生态，但服务器手写"的路线：用 `tokio::net::TcpListener`，不用 `axum::serve`；axum 只用于 `ServerError` 的 `IntoResponse` 和 `StatusCode`。
- `HttpServer::run()` **消费 `self`**（不是 `&self`），这样内部 `Router` 能 move 进 `Arc` 跨 `tokio::spawn` 任务共享。借用检查器因此强制了"先注册路由，再启动"的顺序：`router_mut()` 返回 `&mut Router`，随后 `run()` 消费 `self`。
- `Router` 支持静态（`/api/users`）、动态（`/user/:id`）、通配（`/static/*filepath`）三种模式，并明确区分 **404**（路径不匹配）和 **405**（路径匹配但方法不对）。
- `MAX_REQUEST_SIZE = 64KB`，连接任务的缓冲区在 Keep-Alive 循环外分配一次、跨请求复用；`HttpResponse` 用 `Cow<'static, [u8]>` 让静态响应零拷贝。
- `http/connection.rs` 的 `ConnectionState` / `transition()` 是**纯教学代码**，不要接到真实请求路径上；`infra::socket` 同理（`#[cfg(unix)]`，演示 `unsafe` FFI + `Drop` RAII），不要写依赖它的生产代码。
- 当前 `perf-optimization` 分支，7 项性能优化已完成 6 项（缓冲区复用、字节级解析、`write!` 写 Vec、路径预切分、header/body 边界一次扫描、Cow body），**仅剩"连接读超时"未做**，标记在 `http_server.rs:138-143` 的 `// TODO(perf):`。
- `tests/` 下三个测试文件是空的，`www/` 目录也存在但为空（`main.rs` 已改内联 HTML，`www_root` 暂保留给未来的 `ServeDir`）。

### roze —— Rust 微服务框架（本仓库体量最大的部分）

Cargo workspace，63 个成员：`crates/roze-*` 是能力 crate，`apps/*` 是应用与工具。目前是 **pre-release**，适合评估与内测；是否可用于生产以 `docs/release.md`、`docs/maturity.md`、`docs/production-evidence.md` 为准。

核心设计是 **IDL 优先 + 代码生成**：

- `apps/rozectl` 是代码生成器：从 `.api` 文件生成 REST 服务，从 proto 生成 RPC 服务，还能生成 model（`--orm toasty|sea-orm`）、search 仓库、OpenAPI、TS/JS/Dart 客户端 SDK、Dockerfile、K8s 清单。
- 生成的服务结构固定：REST 为 `src/{main,route,handler,logic,middleware,config,svc,types,openapi}`，RPC 为 `build.rs` + `proto/service.proto` + `src/{lib,client,server,pb,svc,types,logic}`。**应用代码写进 `src/logic`**，这是"约定优于配置"的体现（借鉴 Loco/Rails）。
- `--update` 重新生成时保留应用自有文件：`src/logic/**`、自定义中间件、`config.yaml`、`src/model/<model>_ext.rs`；`--force` 才是全量重建。工作区内的项目要加 `--roze-source path`，否则默认依赖 GitHub 仓库。
- ORM 默认为 Toasty，可用 `--orm sea-orm` 切换；`roze-orm` 只放共享契约（分页、过滤、租户、审计字段、软删除）。
- 并发热路径统一用 DashMap/DashSet：中间件限流与熔断状态、metrics 的带标签状态、singleflight 的 key 查找、rpc 的内存注册表、session/ws/eventbus/mq 的内存索引。
- 热路径 crate 在 `benches/` 下有 Criterion 基准（metrics、local-cache、singleflight、rpc、session/ws/eventbus/mq）。
- 注意：生成的服务固定用 edition **2021**，即使父 workspace 是 2024，以保证生成代码与当前模板对齐；生成的服务默认只带 MySQL/PostgreSQL，避免 `libsqlite3-sys` 的 `links = "sqlite3"` 冲突。

工作区很大，**避免无参 `cargo build` / `cargo test`**，始终用 `-p <crate>` 限定范围。

## 约定与环境

- **代码风格**：偏好 UOP（面向理解编程）——为"人读懂"而写，其次才是机器。保持扁平，函数短（5-10 行），早返回处理异常输入，避免提前抽象和泛型helper（真实重复三次以上再抽），命名用 `动词+名词`，布尔值命名成问句（`isReady` / `hasData` / `canSend`）。注释解释"为什么"和坑点，不解释语法。缩进尽量不超过三层。
- **rust-analyzer**：本机 Windows 用户设置里残留了一条 `C:\Users\12697\...\rust-analyzer.exe`，Remote-SSH 会话也会读到，导致 Linux 上 `spawn ENOENT`。已由远程用户设置（`/home/mofan/.vscode-server/data/User/settings.json` 中的 `"rust-analyzer.server.path": null`）屏蔽，工作区 `.vscode/settings.json` 保持为空、**不要**往里写死 server 路径。若再次出现 ENOENT，优先检查上述两处，而不是改工作区配置。
- **安全**：根目录 `pwd.md` 以明文保存了 git 用户名与访问令牌。不要把它写进任何文档、日志或提交内容；建议加入 `.gitignore`（根目录目前没有 `.gitignore`）。
