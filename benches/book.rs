//! The same benchmarks as matching-engine's Go book (internal/book/bench_test.go), so the
//! two can be compared: three micro benchmarks built identically, and a realistic flow,
//! here the recorded flow of the conformance test.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use orderbook::flow::Flow;
use orderbook::{Book, Order, Side, TimeInForce};

/// A book with `depth` levels of 4 orders on each side, around 10,000.
fn populated(depth: i64) -> Book {
    let mut b = Book::new();
    let mut id = 1;
    for i in 0..depth {
        for _ in 0..4 {
            b.submit(Order::limit(id, Side::Buy, 9_999 - i, 10));
            b.submit(Order::limit(id + 1, Side::Sell, 10_001 + i, 10));
            id += 2;
        }
    }
    b
}

/// One command of the 50,000-command recorded flow (55% passive, 10% aggressive, 5%
/// market, 25% cancels, 5% modifies, with self-trade prevention, post-only and FOK),
/// plus `changes()`, as the Go benchmark does. The book is cleared when the flow wraps.
fn realistic_flow(c: &mut Criterion) {
    let flow = Flow::parse(include_bytes!("../testdata/flow-50k.bin")).expect("flow file");
    let commands = flow.commands;
    c.bench_function("realistic flow (per command)", |b| {
        b.iter_custom(|iters| {
            let mut book = Book::new();
            let mut total = Duration::ZERO;
            let mut done = 0;
            while done < iters {
                book.clear();
                let n = (iters - done).min(commands.len() as u64) as usize;
                let start = Instant::now();
                for cmd in &commands[..n] {
                    black_box(cmd.apply(&mut book));
                    black_box(book.changes());
                }
                total += start.elapsed();
                done += n as u64;
            }
            total
        });
    });
}

/// Add a passive order inside a 1,000-level book, then cancel it.
fn rest_and_cancel(c: &mut Criterion) {
    let mut book = populated(1000);
    let mut id = 1 << 40;
    let mut i = 0;
    c.bench_function("rest and cancel in a 1,000-level book", |b| {
        b.iter(|| {
            id += 1;
            i += 1;
            book.submit(Order::limit(id, Side::Buy, 9_999 - (i % 64), 1));
            black_box(book.cancel(id));
        });
    });
}

/// One IOC order trading with the best resting order.
fn take_top_of_book(c: &mut Criterion) {
    let mut book = populated(100);
    book.submit(Order::limit(1 << 50, Side::Sell, 10_000, 1 << 50));
    let mut id = 1 << 40;
    c.bench_function("take the top of the book (IOC trade)", |b| {
        b.iter(|| {
            id += 1;
            black_box(
                book.submit(Order::limit(id, Side::Buy, 10_000, 1).with_tif(TimeInForce::Ioc)),
            );
        });
    });
}

/// Rest 40 orders on 10 levels, then sweep them with one market order.
fn sweep_ten_levels(c: &mut Criterion) {
    let mut book = Book::new();
    let mut id = 1;
    c.bench_function("rest 40 orders on 10 levels, then sweep them", |b| {
        b.iter(|| {
            for p in 0..10 {
                for _ in 0..4 {
                    book.submit(Order::limit(id, Side::Sell, 10_001 + p, 5));
                    id += 1;
                }
            }
            black_box(book.submit(Order::market(id, Side::Buy, 200)));
            id += 1;
        });
    });
}

criterion_group!(
    benches,
    realistic_flow,
    rest_and_cancel,
    take_top_of_book,
    sweep_ten_levels
);
criterion_main!(benches);
