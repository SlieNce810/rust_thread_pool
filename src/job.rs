//! 任务类型，A3 自己推出来的结论。
//!
//! - FnOnce：任务只能跑一次，调用 Box<dyn FnOnce()> 是就地消费
//! - Send：任务要跨线程移动到 worker 上
//! - 'static：worker 可能比提交方活得更久，不许 borrow 提交方的栈
//! 

pub type Job = Box<dyn FnOnce() + Send + 'static>;