# thread_pool

从零手写一个线程池，用来吃透现代任务调度系统的设计取舍。

这个仓库的规矩是：**每一环先自己推导，推导没完成就不写对应代码**。所以你会看到代码里有 `todo!()` 留白和 TODO 注释——那不是没写完，是还没推到位（推导过程的原始存档在 `notes/`）。

无外部依赖，纯标准库，edition 2024。

## 现在到哪了

| 阶段 | 内容 | 主要模块 | 状态 |
|---|---|---|---|
| 0 | 固定线程数 + 任务队列 + condvar 唤醒 | `pool.rs` / `worker.rs` | 完成（3 个测试绿） |
| 1 | panic 隔离 | `worker.rs` | 完成（`catch_unwind` + `panic_count`） |
| 2 | 结果回传 | `oneshot.rs` | 完成（6 个测试绿） |
| 3 | work stealing | `deque.rs` | 进行中：骨架 + `push` 已落地，`pop` / `steal` 留白 |
| 4 | 每 worker 私有队列 + 窃取调度 | — | 未开始 |
| 5 | 拒绝策略 / 优雅关闭 / Builder | — | 未开始 |

阶段 3 没写完，所以 `cargo test` 里 deque 的测试目前是红的（卡在 `todo!()`），阶段 0–2 的测试是绿的。

## 模块地图

```
src/
  lib.rs       对外只导出 ThreadPool 和 RecvError
  pool.rs      ThreadPool + Shared（任务队列 + Condvar + panic_count）；Drop 是排空语义
  worker.rs    一个 worker = 一个 OS 线程；循环：等任务 → 出锁 → catch_unwind 跑任务
  job.rs       任务类型 = Box<dyn FnOnce() + Send + 'static>
  oneshot.rs   一次性通道，三态 Empty / Filled / Closed，Sender 的 Drop 兜底
  deque.rs     Chase-Lev 双端队列，阶段 3 的核心
  basic.rs     阶段验收测试（#[cfg(test)]）
examples/
  spawn_cost.rs  实测线程创建成本——「为什么要复用线程」的原始数据
```

## 怎么用

```rust
use thread_pool::ThreadPool;

let pool = ThreadPool::new(4);
let handle = pool.submit(|| 21 * 2);
assert_eq!(handle.wait(), Ok(42));
```

三条行为约定，都有测试盯着：

- **Drop 是排空语义**：`drop(pool)` 会先把队列里剩下的任务跑完，worker 才退出，不是丢掉。
- **任务 panic 被隔离**：一个任务炸了，它所在的 worker 继续干活；炸了几次可以从 `pool.panic_count()` 看。
- **任务 panic 时返回值永远不来**：`handle.wait()` 拿到 `Err(RecvError)`，靠 oneshot 的 `Sender::drop` 兜底通知。

## 命令

```bash
cargo test                        # 全部测试
cargo test --lib basic            # 只跑阶段验收测试
cargo run --example spawn_cost    # 实测 spawn + join 的成本（本机基线约 136 µs）
```

## 学习笔记

`notes/` 才是这个项目的主体，代码只是推导的产物：

- `notes/README.md`：笔记索引、复习队列、欠账清单；
- `notes/A*.md`：每一环的问题与结论（A1–A10）；
- `notes/推导问答录.md`：全部推导问答的原始存档，错答也保留。

笔记同时同步在 ima 笔记里（同名一份）。

## 已知留白

- `src/deque.rs`：`pop` 的三个分支（P1）、`steal` 的 CAS 段（P2）还没推导完，是 `todo!()`；
- `src/deque.rs` `push` 里 `bottom.store(b + 1, Relaxed)` 的发布序，与 `steal` 注释里「接住 Release 发布」的说法还没对齐，待想清楚再改；
- `examples/spawn_cost.rs`：预热循环里该跑什么还留着 TODO；
- `src/worker.rs`：`run()` 顶部的推导备忘注释（留白 A 的五条要求）实现完成后还没清。
