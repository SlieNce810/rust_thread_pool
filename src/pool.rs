use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::job::Job;
use crate::oneshot::{self, RecvError, channel};
use crate::worker::Worker;

/// 所有 worker 和所有提交方共享的那一坨状态。
/// 字段用 pub(crate) 而不是私有：worker.rs 是兄弟模块，私有字段它看不见。
pub(crate) struct Shared {
    pub(crate) wake: Condvar,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) panic_count: AtomicUsize,
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
            panic_count: AtomicUsize::new(0),
        });

        let workers = (0..thread_count)
            .map(|_| Worker::spawn(shared.clone()))
            .collect();

        ThreadPool { workers, shared }
    }

    /// 提交任务，并返回一个 TaskHandle<T>。
    /// 任务在线程池中执行，结果通过 oneshot 返回。
    /// job: F
    /// │
    /// │ F: FnOnce() -> T
    /// ▼
    ///let out = job()
    /// │
    /// │ 得到 T
    /// ▼
    ///tx.send(out)
    /// │
    /// │ T 被送进 oneshot
    /// ▼
    ///wrapped 返回 ()
    /// │
    /// ▼
    ///Box<dyn FnOnce() + Send>
    /// │
    /// ▼
    ///线程池 Job 队列
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

        {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.is_shutdown {
                panic!("cannot submit to a shutdown thread pool")
            }
            inner.jobs.push_back(Box::new(wrapped));
        }
        self.shared.wake.notify_one();
        TaskHandle { rx }
    }

    pub fn panic_count(&self) -> usize {
        self.shared.panic_count.load(Ordering::Relaxed)
    }
}


impl Drop for ThreadPool {
    /// 排空语义：把队列里剩下的任务全跑完，worker 才退出。
    /// 不是「丢掉剩下的任务」。
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
        self.rx.recv()
    }
}