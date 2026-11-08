# ADR 0001: orders in an arena, linked by index

**Status:** accepted

## Context

The Go book keeps each price level as an intrusive doubly linked list of orders, so a
cancel unlinks an order in O(1), with an id → node map to find it. A direct translation to
Rust runs into ownership: a node is pointed to by its neighbours, by its level, and by the
index at the same time.

The usual ways around it:

1. **`Rc<RefCell<Node>>` with `Weak` back links.** It compiles, but every access pays a
   reference-count update and a runtime borrow check, every node is a separate heap
   allocation, and a borrow mistake becomes a panic at run time instead of a compile error.
2. **`unsafe` raw pointers.** As fast as Go, with the same bugs possible (dangling pointers,
   use after free), and none of Rust's guarantees for the hardest part of the code.
3. **`std::collections::LinkedList` or `VecDeque` per level.** No O(1) removal from the
   middle: a cancel would search its queue.
4. **An arena.** All nodes live in one `Vec<Node>`; links are `u32` indices into it, with
   `u32::MAX` as "none". Freed slots go on a free list and are reused.

## Decision

Option 4, for orders and for levels.

- **Ownership.** `Book` owns two vectors and nothing else points into them. The borrow
  checker sees plain integer fields, and the crate is `#![forbid(unsafe_code)]`.
- **Memory.** Nodes are contiguous and 48 bytes wide (a unit test pins it); links are 4 bytes instead of 8.
  Reusing freed slots means that, once the vectors have grown to the working size, the
  book does not allocate at all. `tests/allocations.rs` checks this with a counting
  allocator, as the Go version does with `testing.AllocsPerRun`.
- **Levels.** As in Go, each side is a vector of level indices sorted from the worst price
  to the best, so the best level is the last element. `docs/adr/0002-price-levels.md` in
  matching-engine compares this layout with a tree; the same reasoning applies.
- **Index.** `HashMap<OrderId, u32>` with a multiplicative hasher (`hash::IdHasher`), since
  ids are chosen by the venue, not by an attacker who could flood one bucket; `SipHash`
  would cost more than the matching itself.

## Consequences

- **Stale indices.** An index that outlives its order is a logic bug: it points to a reused
  slot. That is the arena's version of a dangling pointer, but it is memory-safe (the
  worst case is wrong data, never undefined behaviour) and it is what
  `Book::check_invariants`, the property tests and the fuzzer look for after every command.
- **No shrinking.** The vectors never shrink. `Book::clear` keeps the memory for the next
  session, which is what a trading system wants; a book that must give memory back would
  need a compaction step that rewrites the indices.
