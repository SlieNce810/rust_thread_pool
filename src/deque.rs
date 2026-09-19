//! Chase-Lev 双端队列——阶段 3 的核心。
//!
//! 固定容量 1024：不扩容，就没有「旧 buffer 还被 thief 读着」的回收难题，
//! 也就没有 ABA。满了怎么办是调用方的事（push 返回 Err，不丢数据）。
//!
//! 三个角色（A10 的三词模型，写代码时对着看）：
//!   bottom → publication  管有没有货（owner 独写，thief 只读）
//!   top    → arbitration  管货归谁（所有 thief + owner 抢最后一件时都写）
//!   slot   → payload      只存堆上那件货的地址（真正的货在堆对象里，见 Q4c）

use std::ptr;
use std::sync::atomic::{AtomicIsize, AtomicPtr, Ordering, fence};

const DEFAULT_CAPACITY: usize = 1024;

pub(crate) struct Deque<T> {
    /// 私账。只有 owner 写（push 时发布 / pop 时预约与回滚），thief 只读。
    bottom: AtomicIsize,
    /// 公账。thief 领号、owner 抢最后一件，全靠 CAS 推进，多写者。
    top: AtomicIsize,
    buffer: Buffer<T>,
}

struct Buffer<T> {
    capacity: usize,
    mask: usize,
    slots: Box<[AtomicPtr<T>]>,
}

// 槽的跨线程读写只发生在协议授权的独占窗口里：
//   写 —— 这个下标还没发布（push：bottom 尚未越过它），或已赢得唯一消费权；
//   读 —— 一定先 CAS 赢得了该逻辑下标的唯一消费权（Q3d）。
// 交接全部经过 bottom/top 的原子协议，所以只要 T: Send，Sync 就成立。
// 注意：是协议+序+CAS 在担保安全（Q4c），AtomicPtr 本身什么都不担保。
unsafe impl<T: Send> Sync for Buffer<T> {}

// 同理：跨线程的数据交接（slot 里的 T）只经协议授权的窗口；
// bottom/top 本身是原子。owner/thief 的角色区分靠调用方纪律
// （worker.rs 里只有 owner 线程调 push/pop，其他线程只许 steal）。
unsafe impl<T: Send> Sync for Deque<T> {}

impl<T> Buffer<T> {
    fn new(capacity: usize) -> Self {
        let slots = (0..capacity)
            .map(|_| AtomicPtr::new(ptr::null_mut()))
            .collect();
        Self {
            capacity,
            mask: capacity - 1,
            slots,
        }
    }

    /// 逻辑下标 → 物理槽。逻辑下标永远单调增长（top/bottom 不回头），
    /// 用 mask 折回环里，所以「下标用完」不存在，只有「环被填满」。
    fn index(&self, i: isize) -> usize {
        (i as usize) & self.mask
    }

    /// 写槽。
    /// # Safety（调用方的两条义务，都出自问答录）
    /// Q3a —— 该下标还没发布给 thief（bottom 尚未越过它）；或
    /// Q3d —— 该下标的唯一消费权已归我。
    fn write(&self, i: isize, value: T) {
        let ptr = Box::into_raw(Box::new(value));
        self.slots[self.index(i)]
            .store(ptr, Ordering::Relaxed);
    }

    /// 只把槽里的地址值抄一份出来，不涉及所有权。
    /// 取货的人先赢得该下标的唯一消费权，再决定要不要 Box::from_raw 接管它。
    fn load_ptr(&self, i: isize) -> *mut T {
        self.slots[self.index(i)]
            .load(Ordering::Relaxed)
    }
}

impl<T> Deque<T> {
    pub(crate) fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "容量必须是 2 的幂：物理槽靠 & mask 环形映射"
        );
        Self {
            bottom: AtomicIsize::new(0),
            top: AtomicIsize::new(0),
            buffer: Buffer::new(capacity),
        }
    }

    /// owner 入队（LIFO 端）。满则原样退回，不丢数据。
    pub(crate) fn push(&self, value: T) -> Result<(), T> {
        let b = self.bottom.load(Ordering::Relaxed);
        let t = self.top.load(Ordering::Acquire);

        if b.wrapping_sub(t) >= self.buffer.capacity as isize {
            return Err(value);
        }

        // ① 真正的数据放到独立堆对象中，
        //    slot 只保存这个对象的地址。
        self.buffer.write(b, value);

        // ② 发布：
        //    thief Acquire 看到新的 bottom 后，
        //    就能看到前面的 slot.store(ptr)。
        self.bottom
            .store(b.wrapping_add(1), Ordering::Release);

        Ok(())
    }

    /// owner 出队（LIFO 端，拿最新的）。
    pub(crate) fn pop(&self) -> Option<T> {
        // ---- 私账阶段 ----
        let b = self.bottom.load(Ordering::Relaxed);
        let t = self.top.load(Ordering::Acquire);

        if b.wrapping_sub(t) <= 0 {
            return None;
        }

        // 先斩后奏：预约 bottom - 1
        let new_b = b - 1;

        self.bottom.store(new_b, Ordering::Release);

        // ---- 可能和 thief 撞车 ----
        fence(Ordering::SeqCst);

        let t = self.top.load(Ordering::Acquire);

        // ① 还有不止一件，owner 稳赢
        if new_b > t {
            let ptr = self.buffer.load_ptr(new_b);

            debug_assert!(!ptr.is_null());

            return Some(unsafe {
                *Box::from_raw(ptr)
            });
        }

        // ② 最后一件，和 thief 通过 top 仲裁
        if new_b == t {
            let won = self
                .top
                .compare_exchange(
                    t,
                    t.wrapping_add(1),
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                )
                .is_ok();

            // 无论谁赢，最后状态都应该恢复为空：
            //
            // top    = t + 1
            // bottom = t + 1
            self.bottom
                .store(new_b.wrapping_add(1), Ordering::Relaxed);

            if won {
                let ptr = self.buffer.load_ptr(new_b);

                debug_assert!(!ptr.is_null());

                return Some(unsafe {
                    *Box::from_raw(ptr)
                });
            }

            return None;
        }

        // ③ new_b < t
        //
        // thief 已经推进了 top，
        // owner 刚才的 bottom-- 预约失败。
        self.bottom
            .store(new_b.wrapping_add(1), Ordering::Relaxed);

        None
    }

    /// thief 偷（FIFO 端，拿最老的）。谁都可以调，包括 owner 自己（测试里就这么用）。
    pub(crate) fn steal(&self) -> Option<T> {
        loop {
            let t = self.top.load(Ordering::Acquire);

            fence(Ordering::SeqCst);

            let b = self.bottom.load(Ordering::Acquire);

            if t >= b {
                return None;
            }

            if self
                .top
                .compare_exchange(
                    t,
                    t.wrapping_add(1),
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                // 赢了 CAS 才去取地址：「先竞争，赢了再记账」。
                // 好处是没赢过就不存在「抄了地址、要不要释放」的问题。
                //
                // 第一层保证地址值是对的，读到的是ptr_t
                // 第二层保证货本身写得完 BOX 里的 T 字段全部可见
                let ptr = self.buffer.load_ptr(t);

                debug_assert!(!ptr.is_null());

                let value = unsafe {
                    *Box::from_raw(ptr)
                };

                return Some(value);
            }

            // CAS 输：这一件被别人先领走了。
            // top 是别人推的，我没改过账、也没取过地址，
            // 所以什么都不用回滚 —— 回到循环顶部重新读 top 再来。
        }
    }

    /// 粗略判空（读旧值最多误报空，不会误报非空……吗？——留给你在阶段 4 想清楚这个 bug 面）。
    pub(crate) fn is_empty(&self) -> bool {
        let t = self.top.load(Ordering::Acquire);
        let b = self.bottom.load(Ordering::Relaxed);
        b.wrapping_sub(t) <= 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;

    /// owner 视角是栈：后进先出。
    #[test]
    fn owner_pop_is_lifo() {
        let q = Deque::new();
        for i in 0..8 {
            assert!(q.push(i).is_ok());
        }
        for expected in (0..8).rev() {
            assert_eq!(q.pop(), Some(expected));
        }
        assert_eq!(q.pop(), None);
    }

    /// thief 视角是队列：先进先出（偷最老的）。
    #[test]
    fn steal_is_fifo() {
        let q = Deque::new();
        for i in 0..8 {
            assert!(q.push(i).is_ok());
        }
        for expected in 0..8 {
            assert_eq!(q.steal(), Some(expected));
        }
        assert_eq!(q.steal(), None);
    }

    /// 固定容量：满了原样退回，不丢数据。
    #[test]
    fn push_full_returns_value_back() {
        let q = Deque::with_capacity(4);
        for i in 0..4 {
            assert!(q.push(i).is_ok());
        }
        assert_eq!(q.push(99), Err(99));
        // 消费一个后又腾出位置
        assert_eq!(q.pop(), Some(3));
        assert!(q.push(100).is_ok());
    }

    /// 锤子测试：owner 一头 pop，3 个 thief 同时偷。
    /// 验的正是协议的安全声明：每件任务恰好被消费一次（不重复、不丢失）。
    /// 这里如果 P1/P2 写错（比如最后一件双双得手），这个测试就是第一批目击者。
    #[test]
    fn concurrent_drain_each_task_consumed_once() {
        const N: usize = 1024;
        let q = Arc::new(Deque::<usize>::new());
        for i in 0..N {
            q.push(i).expect("容量刚好装下 N 件");
        }

        let consumed = Arc::new(AtomicUsize::new(0));
        let double = Arc::new(AtomicUsize::new(0));
        let seen: Arc<Vec<AtomicBool>> =
            Arc::new((0..N).map(|_| AtomicBool::new(false)).collect());

        std::thread::scope(|s| {
            for _ in 0..3 {
                let q = Arc::clone(&q);
                let consumed = Arc::clone(&consumed);
                let double = Arc::clone(&double);
                let seen = Arc::clone(&seen);
                s.spawn(move || loop {
                    match q.steal() {
                        Some(i) => {
                            consumed.fetch_add(1, Ordering::Relaxed);
                            if seen[i].swap(true, Ordering::Relaxed) {
                                double.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        None => break,
                    }
                });
            }
            // owner 同时从另一头拿
            while let Some(i) = q.pop() {
                consumed.fetch_add(1, Ordering::Relaxed);
                if seen[i].swap(true, Ordering::Relaxed) {
                    double.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        assert_eq!(double.load(Ordering::Relaxed), 0, "同一件任务被消费了两次");
        assert_eq!(consumed.load(Ordering::Relaxed), N, "有任务丢了");
    }
}
