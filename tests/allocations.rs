//! Once its buffers have grown, the book must not allocate: the matching path is free of
//! heap traffic. A counting global allocator checks it, per thread, so that the test
//! harness's own allocations on other threads do not count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use orderbook::{Book, Order, Side, TimeInForce};

struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

#[test]
fn steady_state_does_not_allocate() {
    let mut book = Book::new();
    book.submit(Order::limit(1, Side::Sell, 105, 10));
    book.submit(Order::limit(2, Side::Buy, 95, 10));
    let mut id = 100;
    let mut cycle = |book: &mut Book| {
        id += 1;
        book.submit(Order::limit(id, Side::Buy, 100, 3)); // rests
        book.modify(id, 100, 2); // reduce in place
        book.submit(Order::limit(id + 1_000_000, Side::Sell, 100, 1).with_tif(TimeInForce::Ioc)); // trades
        book.cancel(id);
        book.changes();
    };
    for _ in 0..100 {
        cycle(&mut book); // let the buffers grow
    }

    let before = allocations();
    for _ in 0..10_000 {
        cycle(&mut book);
    }
    assert_eq!(allocations() - before, 0, "allocations in steady state");
}
