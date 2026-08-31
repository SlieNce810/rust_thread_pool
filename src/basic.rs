//! 阶段 0 验收：任务真的跑了 / Drop 不丢任务 / 多线程提交不炸。
use thread_pool::ThreadPool;

#[test]
fn submitted_jos_actually_run() {
    let pool = ThreadPool::new(4);
    let (tx, rx) = mpsc::channel();

    for i in 0..8 {
        let tx = tx.clone();
        pool.submit(move || tx.send(i).unwrap());
    }
    drop(tx); // 不 drop 发送端，rx.iter() 永远等不到结束

    let mut got: Vec<i32> = rx.iter().collect();
    got.sort(); // 8 个任务谁先跑完没保证，排序再比
    assert_eq!(got, vec![0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn drop_drains_the_queue() {
    let pool = ThreadPool::new(1);
    let counter = Arc::new(AtomicUsize::new(0));
    let (gate_tx, gate_rx) = mpsc::channel::<()>();

    // 先用 gate 任务占住唯一的 worker，
    // 保证后面 100 个任务在我们 drop 时确实还压在队列里
    pool.submit(move || gate_rx.recv().unwrap());
    for _ in 0..100 {
        let counter = counter.clone();
        pool.submit(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
    }

    gate_tx.send(()).unwrap(); // 不放行 gate，drop 会卡在 join 上
    drop(pool);

    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[test]
fn many_threads_submit_at_once() {
    let pool = Arc::new(ThreadPool::new(4));
    let counter = Arc::new(AtomicUsize::new(0));

    let senders: Vec<_> = (0..8)
        .map(|_| {
            let pool = pool.clone();
            let counter = counter.clone();
            thread::spawn(move || {
                for _ in 0..100 {
                    let counter = counter.clone();
                    pool.submit(move || {
                        counter.fetch_add(1, Ordering::SeqCst);
                    });
                }
            })
        })
        .collect();

    for sender in senders {
        sender.join().unwrap();
    }
    drop(pool); // 8 个发送线程的克隆已随线程结束释放，这里才真正触发 Drop

    assert_eq!(counter.load(Ordering::SeqCst), 800);
}