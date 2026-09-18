# A8 panic 隔离

> 2026-09-06 由阶段 1 实践推导。用户原话，AI 仅做 Markdown 格式整理，未改写内容。

## 1. 问题：一个 Job panic，为什么不能让 worker 跟着死？

线程池里的 worker 是长期存活的资源：

```text
worker
  ↓
取 Job
  ↓
执行
  ↓
再取下一个 Job
```

如果直接执行：

```rust
job();
```

那么 `job` 一旦 panic：

```text
Job panic
   ↓
worker 线程开始 unwind
   ↓
worker 线程退出
```

结果就是：

> 一个用户任务的失败，把整个 worker 永久带走了。

如果线程池有 4 个 worker，连续 4 个任务分别把 4 个 worker 都炸死，那么队列里即使还有正常任务，也可能永远没人执行。

所以需要建立一个明确的**故障隔离边界**：

> Job 可以失败，但不能因为 Job 的 panic 把 worker 一起杀死。

## 2. `catch_unwind` 应该包在哪里？

只包用户任务：

```rust
let result = catch_unwind(AssertUnwindSafe(|| {
    job();
}));
```

不要把整个 worker loop 都包进去。

正确边界：

```text
线程池内部逻辑
    ↓
取出 Job
    ↓
释放队列锁
    ↓
┌────────────────────┐
│ catch_unwind(job)  │
└────────────────────┘
    ↓
继续下一轮
```

为什么只包 `job()`？

因为我们想表达：

```text
用户 Job panic
→ 可以隔离

线程池自己的代码 panic
→ 说明线程池本身可能有 bug
→ 不应该被静默吞掉
```

所以：

> `catch_unwind` 是 Job 和 worker 之间的故障防火墙。

## 3. 怎么判断 Job 炸没炸？

`catch_unwind` 返回：

```rust
Result<T, Box<dyn Any + Send>>
```

这里不需要关心 panic payload 到底是什么。

只需要：

```rust
if result.is_err() {
    // Job panic 了
}
```

也就是只关心一个 bit：

```text
Ok(_)  → 没炸
Err(_) → 炸了
```

例如：

```rust
let result = catch_unwind(AssertUnwindSafe(|| {
    job();
}));

if result.is_err() {
    shared.panic_count.fetch_add(1, Ordering::Relaxed);
}
```

## 4. 为什么需要 `AssertUnwindSafe`？

`catch_unwind` 会担心一件事：

> panic 发生在修改共享状态的中间，程序却继续运行。

例如：

```rust
struct Data {
    list: Vec<i32>,
    is_sorted: bool,
}
```

错误代码：

```rust
data.is_sorted = true;

// 排序进行到一半
panic!("boom");
```

panic 被 catch 以后，程序继续执行，此时可能出现：

```text
is_sorted = true
list      = 半排序状态
```

也就是说：

> panic 把数据的不变量撕开了。

所以 Rust 不愿意默认认定所有闭包在 unwind 后都安全。

`AssertUnwindSafe` 的含义是：

> 我知道 panic 可能中断闭包，但我确认这里接受这个风险。

它不是自动修复器，也不会验证数据是否真的安全。

## 5. 为什么线程池这里可以使用 `AssertUnwindSafe`？

worker 对 Job 的处理是：

```text
拿到 Job
  ↓
执行一次
  ↓
成功或者 panic
  ↓
这个 Job 生命周期结束
```

Job panic 后，线程池不会再次调用：

```rust
job();
```

也不会继续使用 Job 自己的内部状态。

所以在线程池这一层，我们担保的是：

> Job 本身失败后直接报废，worker 只继续处理下一个 Job。

但这不代表 Job 捕获的所有共享数据都自动安全。

例如：

```rust
let data = Arc::new(Mutex::new(Data { ... }));
```

Job panic 后：

```text
Job 被销毁
```

但：

```text
Arc<Data>
```

可能仍然被其他线程持有。

因此：

> `AssertUnwindSafe` 只表示线程池接受 unwind 风险，不表示用户共享状态一定没有被破坏。

## 6. panic 计数器放哪里？

不使用全局：

```rust
static PANIC_COUNT: AtomicUsize
```

否则多个线程池会共用同一个计数器。

更合理的是：

```rust
pub(crate) struct Shared {
    pub(crate) wake: Condvar,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) panic_count: AtomicUsize,
}
```

这样：

```text
ThreadPool A → 自己的 panic_count
ThreadPool B → 自己的 panic_count
```

## 7. 为什么 `panic_count` 不放进 `Inner`？

`Inner` 本身被：

```rust
Mutex<Inner>
```

保护。

但：

```rust
AtomicUsize
```

已经可以安全地被多个 worker 并发修改：

```rust
shared
    .panic_count
    .fetch_add(1, Ordering::Relaxed);
```

不需要再抢 `inner` 的锁。

所以放在：

```rust
Shared
```

本体上更合理。

## 8. 为什么这里用 `Ordering::Relaxed`？

panic_count 只是统计数字：

```text
一共发生了几次 panic
```

我们不依赖：

```text
panic_count 的变化
```

去建立其他数据之间的先后关系。

所以只要求：

> 自增操作本身不能丢。

`AtomicUsize::fetch_add` 已经保证原子性。

因此：

```rust
Ordering::Relaxed
```

足够。

注意这里必须导入：

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
```

不是：

```rust
std::cmp::Ordering
```

后者只有：

```text
Less / Equal / Greater
```

没有：

```text
Relaxed
```

## 9. worker 最终结构

```rust
fn run(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut inner = shared.inner.lock().unwrap();

            while inner.jobs.is_empty() && !inner.is_shutdown {
                inner = shared.wake.wait(inner).unwrap();
            }

            if inner.is_shutdown && inner.jobs.is_empty() {
                return;
            }

            inner.jobs.pop_front().unwrap()
        };

        let result = catch_unwind(AssertUnwindSafe(|| {
            job();
        }));

        if result.is_err() {
            shared
                .panic_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}
```

关键点：

```text
取 Job 时持锁
↓
Job 取出来
↓
锁释放
↓
再执行 catch_unwind(job)
```

绝不能拿着队列锁执行 Job。

否则一个慢任务会把：

```text
其他 worker
submit 方
```

全部堵住。

## 10. 如何证明 panic 没杀死 worker？

测试：

```text
20 个任务
4 个 worker

i % 5 == 0 时 panic
```

panic 的任务：

```text
0
5
10
15
```

共：

```text
4 个
```

正常任务：

```text
16 个
```

所以最终应该得到：

```rust
normal_count == 16
panic_count == 4
```

为什么 `normal_count == 16` 很重要？

如果 Job panic 会把 worker 杀死：

```text
第 1 个 panic → 少一个 worker
第 2 个 panic → 再少一个
第 3 个 panic → 再少一个
第 4 个 panic → 再少一个
```

最坏情况下：

```text
4 个 worker 全死
```

那么后面的正常任务就不会全部完成。

而：

```rust
assert_eq!(normal_count, 16);
```

说明：

> panic 发生后，worker 仍然继续执行后续任务。

这就是这个测试真正证明的东西。

## 11. `join()` 为什么不能静默吞掉错误？

原来的：

```rust
if let Some(handle) = self.handle.take() {
    let _ = handle.join();
}
```

会把：

```text
worker thread panic
```

完全吞掉。

但现在 Job panic 已经被：

```rust
catch_unwind
```

拦住了。

所以如果：

```rust
handle.join()
```

仍然返回：

```text
Err(...)
```

更可能意味着：

> worker 自己的内部逻辑发生了未捕获 panic。

这通常代表线程池 bug，不能静默忽略。

## 12. 为什么也不能在 Drop 路径里 `unwrap()`？

例如：

```rust
handle.join().unwrap();
```

正常情况下如果失败，会产生一个 panic。

但问题是 `ThreadPool::drop()` 可能正运行在另一个 panic 的 unwind 过程中。

例如：

```rust
fn foo() {
    let pool = ThreadPool::new(4);

    panic!("第一次 panic");
}
```

离开作用域时：

```text
第一次 panic
    ↓
stack unwinding
    ↓
自动调用 ThreadPool::drop()
    ↓
worker.join().unwrap()
    ↓
第二次 panic
    ↓
panic during panic
    ↓
process abort
```

因此：

> Drop 路径里主动制造 panic，比普通业务函数里的 panic 危险得多。

## 13. 更合理的 `Worker::join`

```rust
pub fn join(mut self) {
    if let Some(handle) = self.handle.take() {
        if handle.join().is_err() {
            eprintln!("warning: worker thread panicked unexpectedly");
        }
    }
}
```

这样：

```text
正常退出
→ join Ok

worker 内部异常退出
→ join Err
→ 打警告
→ Drop 继续
```

既不会：

```text
静默吞掉真正的线程池错误
```

也不会：

```text
在 Drop 中再次 panic
```

## 14. 最终故障模型

```text
                Job
                 │
                 ▼
        catch_unwind(job)
           /          \
          /            \
       Ok              Err
       │                │
       │          panic_count += 1
       │                │
       └───────┬────────┘
               ▼
          worker 继续
```

如果最后：

```text
worker 本身仍然 panic
```

那么：

```text
ThreadPool::drop()
    ↓
join()
    ↓
Err
    ↓
打印诊断
    ↓
继续清理
```

## 核心结论

### 1. Job panic 和 worker panic 必须分层

```text
Job panic
→ 用户任务失败
→ 可以隔离

worker 内部 panic
→ 线程池自身可能有 bug
→ 应该暴露诊断
```

### 2. `catch_unwind` 只包用户 Job

> 用户代码可以炸，线程池内部逻辑不能被静默吞掉。

### 3. `AssertUnwindSafe` 是人工担保

> 它不是"这里一定安全"，而是"我接受 unwind 后继续执行的风险"。

### 4. panic 计数用 `AtomicUsize`

```text
不需要 Mutex
只做统计
→ Relaxed 足够
```

### 5. Drop 中不要轻易 panic

> Drop 可能运行在另一个 panic 的 unwind 过程中，二次 panic 会直接 abort 进程。

最终浓缩成一句：

> **一个 Job 可以死，但不能把 worker 带死；worker 真死了必须留下诊断，但不能让 Drop 再制造第二次 panic。**

## 这环我卡在哪

（待填——提示：unwind 路径一开始画反了？UnwindSafe 一开始不知道怎么答？把当时的弯路记下来。）
