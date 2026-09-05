use std::{panic::AssertUnwindSafe, sync::{atomic::Ordering, Arc}, thread::{self, JoinHandle}};
use std::panic::catch_unwind;

use crate::pool::Shared;
/// 一个 worker = 一个 OS 线程 + 它的 join 句柄
/// handle 套在 Opting 是因为 JoinHandle::join 按值消费 self，
/// 而 worker 纯在 Vec 里，不 take 出来就拿不走
pub struct Worker {
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(shared:Arc<Shared>) -> Worker {
        let handle = thread::spawn(move || run(shared));
        Worker { handle: Some(handle) }
    }

    pub fn join(mut self) {
        // 故意吞掉 Err：worker 带着 panic 结束是预期内的（catch_unwind 兜不住时），
        // 不是线程池自身的 bug，不该把错误往上抛
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                eprintln!("warning: worker thread pancked unexpectedly");
            }
        }
    }
}

fn run(shared: Arc<Shared>) {
    loop {
        // TODO(你・留白 A)：等任务 + 取任务 + 判断退出
        //
        // 要满足五条：
        //   1. 抢 shared.inner 的锁
        //   2. 没任务 且 没关闭 → 睡（shared.wake.wait，记得放 while 里）
        //   3. 醒了发现「已关闭 且 队列空」→ return
        //   4. 否则 pop_front 一个任务
        //   5. 先出锁，再跑任务 —— 拿着锁跑任务会把所有提交方堵死

        let job = {
            // 1. 抢 shared.inner 的锁
            let mut inner = shared.inner.lock().unwrap(); 
            // 2. 没任务 且 没关闭 → 睡（shared.wake.wait，记得放 while 里）
            while inner.jobs.is_empty() && !inner.is_shutdown {  
                inner = shared.wake.wait(inner).unwrap();
            }

            // 3. 醒了发现「已关闭 且 队列空」→ return
            if inner.is_shutdown && inner.jobs.is_empty() {
                return;
            };
            // 4. 否则 pop_front 一个任务
            inner.jobs.pop_front().unwrap()
        };

        let result = catch_unwind(AssertUnwindSafe(|| {
            job();
        }));

        if result.is_err() {
            shared.panic_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}