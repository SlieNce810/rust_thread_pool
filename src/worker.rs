use std::{sync::{Arc, Condvar}, thread::{self, JoinHandle}};

use crate::{job, pool::Shared};
/// 一个 worker = 一个 OS 线程 + 它的 join 句柄
/// handle 套在 Opting 是因为 JoinHandle::join 按值消费 self，
/// 而 worker 纯在 Vec 里，不 take 出来就拿不走
pub struct Worker {
    pub(crate) handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(shared:Arc<Shared>) -> Worker {
        let handle = thread::spawn(move || run(shared));
        Worker { handle: Some(handle) }
    }

    pub fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
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
        job();
    }
}