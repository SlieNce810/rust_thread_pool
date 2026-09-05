//! oneshot：一次性通道，把任务的返回值从 worker 送回提交方。
//! 和 Stage 0 的任务队列同一个模子：共享缓冲 + 锁 + 「有变化」的信号，
//! 只是货从「任务」换成了「值」，且最多一件。

use std::sync::{Arc, Condvar, Mutex};

/// 槽的三状态。用 enum 而不是「Option + bool」两个字段：
/// 三个状态互斥，「有值且已关闭」这种说谎组合在类型上写不出来。
pub(crate) enum Slot<T> {
    Empty,
    Filled(T),
    Closed,
}

pub(crate) struct Inner<T> {
    slot: Mutex<Slot<T>>,
    wake: Condvar,
}

pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

/// 失败原因只有一种（发送端死了，值永远不会来），空结构体足够。
/// 不用 std 的 mpsc::RecvError：那是 mpsc 的错误类型，语义绑死在「通道关闭」上，
/// 我们自己的 API 自己命名。
#[derive(Debug, PartialEq, Eq)]
pub struct RecvError;

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        slot: Mutex::new(Slot::Empty),
        wake: Condvar::new(),
    });

    (
        Sender {
            inner: Arc::clone(&inner),
        },
        Receiver { inner },
    )
}

impl<T> Sender<T> {
    /// 参数是 self 不是 &self——调用即消费，编译器保证只发一次。
    pub fn send(self, value: T) {
        let Inner { slot, wake } = &*self.inner;
        let mut guard = slot.lock().unwrap();
        *guard = Slot::Filled(value);
        drop(guard); // 先放锁再喊人：醒来的等待方立刻要抢锁，别让它等我们的
        wake.notify_one();
        // send 结束 self 被 drop，Drop 会执行——但它见到槽是 Filled（不是 Empty），
        // 自动无事发生。不需要「防止 Drop 弄坏数据」的代码，Empty 判别就是防线。
    }
}

impl<T> Drop for Sender<T> {
    /// 发送端死了（包括 panic 中途死）：把「值永远不来了」留给等待方。
    /// 只有槽是 Empty 才置 Closed——已经 Filled 说明值送达过，什么都不做。
    fn drop(&mut self) {
        {
            let mut guard = self.inner.slot.lock().unwrap();
            if matches!(*guard, Slot::Empty) {
                *guard = Slot::Closed;
            }
        }
        self.inner.wake.notify_one();
    }
}

impl<T> Receiver<T> {
    /// 等值。Ok(v)＝等到了；Err(RecvError)＝发送端死了，值永远不会来。
    pub fn recv(self) -> Result<T, RecvError> {
        // 从 Arc 里借出两个成员：slot 是 &Mutex（锁本身），wake 是 &Condvar。
        // 注意区分：Mutex 是锁，lock() 返回的 guard 才是「锁住后的门」。
        let Inner { slot, wake } = &*self.inner;
        let mut guard = slot.lock().unwrap();

        while matches!(*guard, Slot::Empty) {
            // 空槽才睡；Filled / Closed 都该醒。wait 原子地放锁+入睡，醒来重新拿到锁。
            guard = wake.wait(guard).unwrap();
        }
        match std::mem::replace(&mut *guard, Slot::Closed) {
            Slot::Filled(v) => Ok(v),
            Slot::Closed => Err(RecvError),
            // 出循环必非 Empty——while 条件的直接推论，所以 unreachable 安全。
            Slot::Empty => unreachable!(),
        }
    }
}
