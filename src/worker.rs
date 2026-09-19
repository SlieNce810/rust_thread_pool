//! worker：一个 OS 线程 + 一个本地 deque。
//!
//! 找活顺序（H1 的答案位置，填之前把每步的「为什么在这」想清楚）：
//!   ① 自己的 deque（bottom 端 pop——最便宜，无锁无争用，数据还热）
//!   ② 全局注入队列（一把锁，人人可达；批量搬一批回本地，摊薄锁的次数）
//!   ③ 偷别人的 deque（top 端 steal——最贵，所以偷回来的必须是大粒度老任务才值回票价）
//! 三步全空 → 睡 condvar（阶段 0 的老机器原样复用）。

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::job::Job;
use crate::pool::Shared;

thread_local! {
    /// 只在 pool 的 worker 线程上有值。
    /// submit 的快路径（H0）和 wait() 的帮助循环（H4）都靠它回答「我现在是不是 worker」。
    static WORKER_CTX: RefCell<Option<WorkerCtx>> = const { RefCell::new(None) };
}

pub(crate) struct WorkerCtx {
    pub(crate) shared: Arc<Shared>,
    pub(crate) index: usize,
}

/// 在 worker 线程上执行 f(Some(ctx))，普通线程上执行 f(None)。
pub(crate) fn with_worker_ctx<R>(f: impl FnOnce(Option<&WorkerCtx>) -> R) -> R {
    WORKER_CTX.with(|c| f(c.borrow().as_ref()))
}

pub struct Worker {
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn spawn(shared: Arc<Shared>, index: usize) -> Worker {
        let handle = std::thread::spawn(move || run(shared, index));
        Worker { handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                eprintln!("warning: worker thread panicked unexpectedly");
            }
        }
    }
}

fn run(shared: Arc<Shared>, index: usize) {
    // 登记：本线程从此是 deques[index] 的 owner（push/pop 只许自己调，别人只能 steal）。
    WORKER_CTX.with(|c| {
        *c.borrow_mut() = Some(WorkerCtx {
            shared: shared.clone(),
            index,
        })
    });

    loop {
        if let Some(job) = find_work(&shared, index) {
            run_one(&shared, index, job);
            continue;
        }

        // 没活：老机器——持锁检查「全局空 && 自己 deque 空 && 没关」，不满足就睡。
        // 想清楚：条件里为什么没有「别人的 deque」？
        //   别人的 deque 是别人的私账——他在不在干活由他自己判断；
        //   而你睡着时只有两件事能叫醒你：外部 submit（notify_one）或关闭（notify_all）。
        //   「别的 worker deque 堆了货」叫不醒你是本阶段的已知惰性，
        //   修它（定向唤醒）是阶段 4 的活。
        let mut inner = shared.inner.lock().unwrap();
        while inner.jobs.is_empty() && !inner.is_shutdown && shared.deques[index].is_empty() {
            inner = shared.wake.wait(inner).unwrap();
        }
        // 退出条件：关了，且全局空，且自己 deque 空（排空语义：睡醒也要把手头清完才走）。
        if inner.is_shutdown && inner.jobs.is_empty() && shared.deques[index].is_empty() {
            return;
        }
    }
}

/// 执行一件任务：隔离 panic、记账。阶段 1 的老逻辑 + executed 计数（直方图验收用）。
pub(crate) fn run_one(shared: &Shared, index: usize, job: Job) {
    let result = catch_unwind(AssertUnwindSafe(|| job()));
    if result.is_err() {
        shared.panic_count.fetch_add(1, Ordering::Relaxed);
    }
    shared.executed[index].fetch_add(1, Ordering::Relaxed);
}

/// 找一件活。三步的顺序是 H1；批量大小是 H3；偷谁家是 H2。
/// H4（wait 的帮助循环）也会调它——所以是 pub(crate)。
pub(crate) fn find_work(shared: &Shared, index: usize) -> Option<Job> {
    todo!(
        "H1：三步按「本地 → 全局 → 偷」排。填之前先答自己：
         为什么 own pop 必须排最前？（最便宜 + 无争用 + 缓存热）
         为什么 steal 必须排最后？（一次 steal 一趟 CAS，是全池最贵的取货方式，
         只有本地和全局都枯竭时才值得付这个价）

         ① let job = shared.deques[index].pop();
            拿到直接 return Some(job)。

         ② 全局队列：lock shared.inner，取 guard；
            有货就搬一批进自己 deque——H3：搬多少？
            提示：搬太少，锁的次数降不下来；搬太多，货堆在自己手里、
            别人既偷不动你也轮不到全局。「搬现有的一半」是经典答案，
            想想它为什么自带均衡（下一个人拿剩下的一半的一半……）。
            实现：循环 inner.jobs.pop_front() → deques[index].push(job)，
            push 返回 Err（自己 deque 满 1024）就把手上这件 inner.jobs.push_front() 回去并停止；
            搬完 drop(guard)（先放锁！）再 deques[index].pop()。
            注意：is_shutdown 不拒绝干活——排空语义，只是不再等新任务。

         ③ 偷：从 (index + 1) % n 开始逐个试别人的 deque.steal()，绕开自己。
            H2：为什么起点是「自己下一位」而不是永远从 0 开始？
            （所有 thief 挤同一个 victim = 把全局锁的争用原样搬到 victim 的 top 端）
            偷到一件：shared.steal_count.fetch_add(1, Ordering::Relaxed)——
            验收测试就盯这个数。全空 return None。"
    )
}
