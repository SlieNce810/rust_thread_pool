//! 阶段 3 下半场验收：窃取真的发生了、负载真的摊开了、fork-join 真的不死锁。

use std::sync::Arc;
use std::time::Duration;

use crate::ThreadPool;

/// 经典的「帮助型」并行求和：右半当前线程直接算，左半提交给池子。
/// 从 worker 里递归 submit 时，子任务走 H0 快路径进自己的 deque，
/// 父任务的 wait() 走 H4 帮助循环——没有这两条路，这个测试就是死锁挂死。
fn par_sum(pool: &Arc<ThreadPool>, mut v: Vec<i64>) -> i64 {
    if v.len() <= 1024 {
        return v.iter().sum();
    }
    let right = v.split_off(v.len() / 2);
    let left = v;

    let p = Arc::clone(pool);
    let handle = pool.submit(move || par_sum(&p, left));

    let right_sum = par_sum(pool, right); // join 的经典形状：一半就地干
    right_sum + handle.wait().unwrap()
}

/// 验收 ①：偷真的发生了（steal_count > 0），且没有一个 worker 全程闲死。
/// 手法：一个外部线程快速倾倒 4000 个轻重混合任务——
/// 批量搬运（H3）必然造成「有人囤货、有人断粮」，断粮的只能去偷。
#[test]
fn stealing_moves_work_from_busy_to_idle() {
    let pool = ThreadPool::new(4);

    let mut handles = Vec::new();
    for i in 0..4000u64 {
        let dur = if i % 97 == 0 {
            Duration::from_millis(2) // 少数重活：制造忙闲差
        } else {
            Duration::from_micros(30) // 大量轻活：制造囤货
        };
        handles.push(pool.submit(move || std::thread::sleep(dur)));
    }
    for h in handles {
        h.wait().unwrap();
    }

    let hist = pool.executed();
    let stolen = pool.steal_count();
    println!("executed 直方图 = {hist:?}, stolen = {stolen}");

    assert!(
        hist.iter().all(|&c| c > 0),
        "有 worker 全程没摸到活: {hist:?}"
    );
    assert!(stolen > 0, "一次都没偷到——窃取循环（H1–H3）没接上");
}

/// 验收 ②：递归 fork-join + 阻塞等待子任务，不死锁。
/// 「只有全局队列」的世界里这会死锁（自己等自己）；本地 deque + 帮助循环让它跑通。
/// 如果这个测试挂死，就是在告诉你 H4 写错了——挂死本身就是失败信号。
#[test]
fn recursive_fork_join_does_not_deadlock() {
    let pool = Arc::new(ThreadPool::new(4));

    let data: Vec<i64> = (1..=1_000_000i64).collect();
    let expected: i64 = data.iter().sum();

    let p = Arc::clone(&pool);
    let h = pool.submit(move || par_sum(&p, data));

    assert_eq!(h.wait().unwrap(), expected);
    println!("fork-join steal_count = {}", pool.steal_count());
}
