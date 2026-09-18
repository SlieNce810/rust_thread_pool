//! Chase-Lev 双端队列——阶段 3 的核心。
//!
//! 固定容量 1024：不扩容，就没有「旧 buffer 还被 thief 读着」的回收难题，
//! 也就没有 ABA。满了怎么办是调用方的事（push 返回 Err，不丢数据）。
//!
//! 三个角色（A10 的三词模型，写代码时对着看）：
//!   bottom → publication  管有没有货（owner 独写，thief 只读）
//!   top    → arbitration  管货归谁（所有 thief + owner 抢最后一件时都写）
//!   slot   → payload      真正的货（plain 读写，安全性由协议保证，见 Q4c）

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{fence, AtomicIsize, Ordering};

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
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
}

// 槽的跨线程读写只发生在协议授权的独占窗口里：
//   写 —— 这个下标还没发布（push：bottom 尚未越过它），或已赢得唯一消费权；
//   读 —— 一定先 CAS 赢得了该逻辑下标的唯一消费权（Q3d）。
// 交接全部经过 bottom/top 的原子协议，所以只要 T: Send，Sync 就成立。
// 注意：是协议+序+CAS 在担保安全（Q4c），UnsafeCell 本身什么都不担保。
unsafe impl<T: Send> Sync for Buffer<T> {}

// 同理：跨线程的数据交接（slot 里的 T）只经协议授权的窗口；
// bottom/top 本身是原子。owner/thief 的角色区分靠调用方纪律
// （worker.rs 里只有 owner 线程调 push/pop，其他线程只许 steal）。
unsafe impl<T: Send> Sync for Deque<T> {}

impl<T> Buffer<T> {
    fn new(capacity: usize) -> Self {
        let slots: Vec<UnsafeCell<MaybeUninit<T>>> = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        Self {
            capacity,
            mask: capacity - 1,
            slots: slots.into_boxed_slice(),
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
    unsafe fn write(&self, i: isize, value: T) {
        unsafe { (*self.slots[self.index(i)].get()).write(value); }
    }

    /// 读槽并拿走 T。
    /// # Safety
    /// 必须先 CAS 赢得该逻辑下标的唯一消费权（Q3d），
    /// 且该下标已被发布（Q3a）——否则会读到半初始化的货，或同一件货被读两次。
    unsafe fn read(&self, i: isize) -> T {
        unsafe { (*self.slots[self.index(i)].get()).assume_init_read() }
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
        let b = self.bottom.load(Ordering::Relaxed); // 读自己的私账：没人和我抢写，读哪个序都行
        let t = self.top.load(Ordering::Acquire); // 公账只为算余量；top 只前进，读旧了只会更保守
        if b.wrapping_sub(t) >= self.buffer.capacity as isize {
            return Err(value);
        }

        todo!(
            "留白 P0（对照问答录 Q3a，就两步，顺序是全部内容）：
             ① 把货写进槽：unsafe {{ self.buffer.write(b, value) }}（plain 写——为什么可以？Q4c）
             ② 发布：bottom.store(b + 1, ??)。
                问自己：这一步是在向 thief 发布「slot 里有货了」吗（publication）？
                该用什么序？（你修正过我的那个点，就在这里落地）
             然后 return Ok(())"
        )
    }

    /// owner 出队（LIFO 端，拿最新的）。
    pub(crate) fn pop(&self) -> Option<T> {
        // ---- 私账阶段：动 bottom 不需要和任何人商量（Q3b）----
        let b = self.bottom.load(Ordering::Relaxed);
        let t = self.top.load(Ordering::Acquire);
        if b.wrapping_sub(t) <= 0 {
            return None; // 空。此刻还没有任何「可能撞车」的动作，不需要 fence（Q4a 想清楚这点）
        }

        // 乐观预约：先把最后一个位置划到自己名下（Q3b「先斩后奏」）
        let new_b = b - 1;
        self.bottom.store(new_b, Ordering::Release); // 收缩也在发布状态：thief 的「还有没有货」判断读的是它

        // ---- 公账阶段：从这行起，可能和 thief 撞车（Q4a 的战场）----
        fence(Ordering::SeqCst); // 至少一方必须看到足够新的对方，打破「双赢」（Q4a）
        let t = self.top.load(Ordering::Acquire);

        todo!(
            "留白 P1（三个分支，对照问答录 Q3c / Q3d / Q3e）：
             分支一 new_b > t：两端不撞车，这件货的独占权来自我的预约（thief 够不到 new_b）
               → unsafe {{ let job = self.buffer.read(new_b); Some(job) }}
             分支二 new_b == t：只剩这一件，公账仲裁
               → CAS(top, t → t+1)：赢了读走；输了不读槽，但私账 new_b 已经划出去了，
                 必须修回一个值让「空队列」重新自洽（修回几？想一想 thief 赢后 top 变成了几）
             分支三 new_b < t：thief 已经偷穿了，我扑空 → 私账同样要修回去，return None"
        )
    }

    /// thief 偷（FIFO 端，拿最老的）。谁都可以调，包括 owner 自己（测试里就这么用）。
    pub(crate) fn steal(&self) -> Option<T> {
        // thief 的一切从公账开始（Q3d：先取号，再确认有货）
        let t = self.top.load(Ordering::Acquire);
        fence(Ordering::SeqCst); // Q4a：不许双方各自基于旧世界同时宣布胜利
        let b = self.bottom.load(Ordering::Acquire); // Acquire = publication 的接收端：
                                                     // 接住 owner push 对 bottom 的 Release 发布，
                                                     // slot 里的货才能 happens-before 到我手里
                                                     //（Q4b：修正后落地的位置）
        if t >= b {
            return None; // 空。读到旧值没关系（允许 stale），后面的 CAS 兜底
        }

        todo!(
            "留白 P2（对照问答录 Q3d / Q3e）：
             ① CAS(top, t → t+1)：先竞争，赢了才记账。
                成功用 AcqRel——想想这个 CAS 同时是一次 acquire（接住 top 上的发布链）
                和一次 release（把「我赢了」发布给别人）；失败 Relaxed。
             ② 赢了：这才第一次碰 payload——unsafe {{ let job = self.buffer.read(t); Some(job) }}
             ③ 输了：直接 return None。
                想清楚 Q3e：为什么 thief 输了什么都不用回滚？（你答过：先竞争，赢了再记账）"
        )
    }

    /// 粗略判空（读旧值最多误报空，不会误报非空……吗？——留给你在阶段 4 想清楚这个 bug 面）。
    pub(crate) fn is_empty(&self) -> bool {
        let t = self.top.load(Ordering::Acquire);
        let b = self.bottom.load(Ordering::Relaxed);
        b.wrapping_sub(t) <= 0
    }
}
