use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::deque::Deque;
use crate::job::Job;
use crate::oneshot::{self, RecvError, channel};
use crate::worker::{with_worker_ctx, Worker};

pub(crate) struct Shared {
    pub(crate) wake: Condvar,
    /// 全局注入队列：外部线程的提交入口（阶段 0 的 inner 原样保留）。
    pub(crate) inner: Mutex<Inner>,
    /// 每人一个本地 deque。deques[i] 的 owner 是 worker i：
    /// 只有 worker i 的线程 push/pop，其他 worker 只许 steal——
    /// D2 的所有权纪律在这里落成代码。
    pub(crate) deques: Vec<Deque<Job>>,
    pub(crate) panic_count: AtomicUsize,
    /// 验收指标：偷成功的次数。tasks_stolen > 0 是阶段 3 的验收线。
    pub(crate) steal_count: AtomicUsize,
    /// 验收指标：每个 worker 执行了几件。直方图。
    pub(crate) executed: Vec<AtomicUsize>,
}

pub(crate) struct Inner {
    pub(crate) jobs: VecDeque<Job>,
    pub(crate) is_shutdown: bool,
}

pub struct ThreadPool {
    workers: Vec<Worker>,
    shared: Arc<Shared>,
}

pub struct TaskHandle<T> {
    rx: oneshot::Receiver<T>,
}

impl ThreadPool {
    /// 固定 thread_count 个 worker。Builder（可配置线程名、栈大小）留到阶段 5。
    pub fn new(thread_count: usize) -> ThreadPool {
        assert!(thread_count > 0, "线程池最少要有一个 worker");

        let shared = Arc::new(Shared {
            wake: Condvar::new(),
            inner: Mutex::new(Inner {
                jobs: VecDeque::new(),
                is_shutdown: false,
            }),
            deques: (0..thread_count).map(|_| Deque::new()).collect(),
            panic_count: AtomicUsize::new(0),
            steal_count: AtomicUsize::new(0),
            executed: (0..thread_count).map(|_| AtomicUsize::new(0)).collect(),
        });

        let workers = (0..thread_count)
            .map(|i| Worker::spawn(shared.clone(), i))
            .collect();

        ThreadPool { workers, shared }
    }

    /// 提交任务，返回 TaskHandle<T>。旧调用方零改动（T 推断成 () 的老兼容依然成立）。
    pub fn submit<F, T>(&self, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = channel();

        let wrapped = move || {
            let out = job();
            tx.send(out);
        };

        // H0 路由——填完它，旧的 6 个测试就该恢复绿。
        // 两条路最终都把 Box::new(wrapped) 送进某个队列；尾巴 TaskHandle { rx } 共用。
        with_worker_ctx(|ctx| match ctx {
            None => {
                // 外部线程 → 全局队列。
                todo!(
                    "H0-外部（老路径照搬）：
                     锁 inner；is_shutdown 就 panic!（老语义）；
                     jobs.push_back(Box::new(wrapped))；放锁；wake.notify_one()。"
                )
            }
            Some(ctx) => {
                // worker 线程 → 快路径。
                todo!(
                    "H0-快：ctx.shared.deques[ctx.index].push(Box::new(wrapped))
                     ——无锁、不碰全局队列、不检查 is_shutdown。落键盘前想清楚三件事：
                     ① 为什么敢不查 shutdown？提示在 worker 的退出条件里：
                        「shutdown && 全局空 && 自己 deque 空」——worker 睡前会把自己 deque 排空，
                        所以从它身上 submit 的任务永远不会被丢，Drop 的排空语义不破。
                     ② 要不要 notify？睡着的同伴偷得到你的 deque，但本阶段可以先不 notify：
                        等结果的父任务靠 H4 帮助循环自己消化自己的货。
                        （定向唤醒是阶段 4 的活。）把这个取舍写成注释留在这。
                     ③ push 满了（1024）：Err(job) 退回全局队列老路径，兜底不丢任务。"
                )
            }
        });

        TaskHandle { rx }
    }

    pub fn panic_count(&self) -> usize {
        self.shared.panic_count.load(Ordering::Relaxed)
    }

    pub fn steal_count(&self) -> usize {
        self.shared.steal_count.load(Ordering::Relaxed)
    }

    /// 每个 worker 执行了多少件，验收直方图用。
    pub fn executed(&self) -> Vec<usize> {
        self.shared
            .executed
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .collect()
    }
}

impl Drop for ThreadPool {
    /// 排空语义：全局队列 + 各自本地 deque 全部跑完，worker 才退出。
    fn drop(&mut self) {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.is_shutdown = true;
        }
        self.shared.wake.notify_all();

        for worker in self.workers.drain(..) {
            worker.join();
        }
    }
}

impl<T> TaskHandle<T> {
    pub fn wait(self) -> Result<T, RecvError> {
        // 外部线程：老行为，直接睡在 oneshot 上。
        // worker 线程：进帮助循环（H4）。
        let on_worker = with_worker_ctx(|ctx| ctx.is_some());
        if !on_worker {
            return self.rx.recv();
        }

        todo!(
            "H4 帮助循环——下半场的皇冠。填完它，fork-join 测试才亮。
             死锁的根源先想透：我阻塞在 oneshot 上，而我的结果
             可能正躺在我自己的 deque 里（我 submit 的子任务）——
             我睡死，就没人跑它。所以等的时候不能干等，循环做三件事：

             ① self.rx.try_recv() 有结论 → 返回它
                （Some(Ok(v)) → Ok(v)；Some(Err(e)) → Err(e)）
             ② with_worker_ctx 拿到 ctx（这里必是 Some），
                worker::find_work(&ctx.shared, ctx.index) 找一件活
                → 找到了就 worker::run_one(&ctx.shared, ctx.index, job) 跑掉它，
                回 ① 再看一眼结果。
                注意 find_work 第一步就是 own pop——你的子任务优先被你自己跑掉，
                这就是 D1c 里 LIFO 深度优先在「等待」上的兑现。
             ③ 全世界暂时没活、结果还没来 → std::thread::yield()，回 ①。
                别睡死（结果正在别的核上算），也别空转烧 CPU。
             边界：帮助循环不检测「两个任务互相等」的环——那是用户 bug，
             池子的态度是无限帮忙干活而不是死锁，注释里写明。"
        )
    }
}
