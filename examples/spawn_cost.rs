use std::{hint::black_box, thread, time::Instant};


const ROUNDS:u32 = 10_000;
const WARMUP:u32 = 1_000;

/// 任务本体：故意做得极简，这样测出来的差价才全归线程
fn task() -> u64 {
    let mut acc = 0u64;
    acc += 1;
    black_box(acc) // black_box 拦住优化器，让 acc 真的被算出来
}

fn main() {
    // ---- 1. 预热：把缺页、TLS 初始化这些一次性成本先付掉 ----
    // TODO(你・留白 1)：预热循环里该跑什么？
    // 提示：下面第 2、3 组分别要测哪两件事，预热就先把它们各跑一遍
    for _ in 0..WARMUP {
        let handle = thread::spawn(task);
        black_box(handle.join().unwrap());
    }


    // ---- 2. 测「交给线程跑」：spawn + 等它结束 ----
    let start = Instant::now();
    for _ in 0..ROUNDS {
        let handle = thread::spawn(task);
        black_box(handle.join().unwrap()); // 吃掉返回值，防止整块被优化掉
    }
    let spawn_ns = start.elapsed().as_nanos() as f64 / ROUNDS as f64;


    // ---- 3. 测「直接跑」：同一个 task，就在当前线程调用 ----
    let start = Instant::now();
    for _ in 0..ROUNDS {
        black_box(task());
    }
    let call_ns = start.elapsed().as_nanos() as f64 / ROUNDS as f64;

    println!("spawn + join : {:>10.1} ns", spawn_ns);
    println!("直接调用     : {:>10.1} ns", call_ns);
    println!("倍数         : {:>10.1} x", spawn_ns / call_ns);
}