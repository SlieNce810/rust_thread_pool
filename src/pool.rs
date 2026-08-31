

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::job::Job;
use crate::worker::Worker;

/// 所有 worker 和所有提交方共享的那一坨状态。
/// 字段用 pub(crate) 而不是私有：worker.rs 是兄弟模块，私有字段它看不见。
pub(crate) struct Shared {
    pub(crate) wake: Condvar,
    pub(crate) inner: Mutex<Inner>,
}

pub(crate) struct Inner {
    pub(crate) jobs: VecDeque<Job>,
    pub(crate) is_shutdown: bool,
}

pub struct ThreadPool {
    workers: Vec<Worker>,
    shared: Arc<Shared>,
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
        });

        let workers = (0..thread_count)
            .map(|_| Worker::spawn(shared.clone()))
            .collect();

        ThreadPool { workers, shared }
    }

    /// 提交任务。阶段 0 不返回结果（oneshot 是阶段 2 的事），
    /// 也不管「关闭后还提交」的情况（三态关闭是阶段 5 的事）。
    pub fn submit<F>(&self, job: F)
    where 
        F:FnOnce() + Send() + 'static,
    {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.jobs.push_back(Box::new(job));
        }
    }    

    // TODO(你・留白 B)：唤醒 worker。
    // 选 notify_one 还是 notify_all？说清楚你选它的理由。
    unimplemented!()
}

impl Drop for ThreadPool {
    /// 排空语义：把队列里剩下的任务全跑完，worker 才退出。
    /// 不是「丢掉剩下的任务」。
    fn drop(&mut self) {
        // TODO(你・留白 C)：置 is_shutdown → 唤醒 worker → join 等它们排空退出
        //
        // 两个坑先想清楚：
        //   - 置标志必须持锁，否则和 worker 的「检查标志 + 入睡」错开，就是经典丢唤醒
        //   - 只 notify_one 的话，剩下那几个 worker 会永远睡下去，join 直接卡死
        unimplemented!()
    }
}