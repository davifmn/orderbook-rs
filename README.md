# orderbook-rs

[![CI](https://github.com/davifmn/orderbook-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/davifmn/orderbook-rs/actions/workflows/ci.yml)

A limit order book in Rust, ported from the book of
[matching-engine](https://github.com/davifmn/matching-engine) (Go) and **checked against
it**. The Go implementation writes a recorded flow of 50,000 commands. Replaying it here
must give the same events, field by field, and the same final digest of the book.

- **Rules.**
  - Price-time priority, at the resting order's price.
  - Limit and market orders; GTC, IOC and FOK; post-only.
  - Cancel and modify: reducing the quantity keeps the queue position, anything else
    re-queues.
  - Self-trade prevention in three modes, which fill-or-kill accounts for exactly.
- **Safe and dependency-free.** `#![forbid(unsafe_code)]` and no runtime dependencies.
  Orders live in an arena linked by `u32` indices, not `Rc<RefCell<…>>` or raw pointers
  ([ADR 0001](docs/adr/0001-arena-and-indices.md)).
- **No heap traffic.** Once its buffers have grown, the book does not allocate; a counting
  allocator checks it in the tests.
- **Integer prices and quantities.** Ticks and lots: no floating point anywhere.
- **Illegal states are unrepresentable.** An invalid side, order type or self-trade mode
  cannot be built, so three of the Go version's runtime checks do not exist here.

```rust
use orderbook::{Book, EventKind, Order, Side, TimeInForce};

let mut book = Book::new();
book.submit(Order::limit(1, Side::Sell, 3415, 500));                      // rests
let events = book.submit(Order::limit(2, Side::Buy, 3420, 300).with_tif(TimeInForce::Ioc));
assert_eq!(events[1].kind, EventKind::Trade);                             // 300 @ 3415, the maker's price
assert_eq!(book.best_level(Side::Sell).unwrap().qty, 200);
```

## How it is checked

| Check | What |
|---|---|
| **Conformance with Go** (`tests/conformance.rs`) | 50,000 recorded commands from matching-engine's realistic flow, with accounts, all three STP modes, post-only and FOK orders added (3,398 self-trade cancels, 363 post-only and 287 FOK rejections, 16,372 trades). The number of events and trades, an FNV-1a hash of every event field (with checkpoints every 1,000 commands to locate a difference) and the final book digest must equal what the Go book produced ([ADR 0002](docs/adr/0002-conformance-with-go.md)) |
| **Property tests** (`tests/against_reference.rs`) | `proptest` sequences over a narrow price band with few ids and owners, run on the book and on a naive reference model (one unsorted `Vec`, linear scans, FOK by trial on a copy): identical events and resting orders, and the book's invariants, after every command |
| **Fuzzing** (`fuzz/`) | The same check under `cargo-fuzz` (libFuzzer, coverage guided), 2 minutes per CI run |
| **Rules** (`tests/rules.rs`) | Each rule on its own: priority, every order type, every STP mode, modify semantics, validation, level changes, depth, digest |
| **Allocations** (`tests/allocations.rs`) | A counting global allocator: zero allocations over 10,000 rest / reduce / trade / cancel cycles |
| **CI** | rustfmt, clippy pedantic with `-D warnings`, tests in release, doc tests, docs without warnings, the library on the minimum Rust version (1.85), fuzzing, benchmarks, `cargo audit` |

## Performance

Criterion benchmarks (`benches/book.rs`) mirror the Go benchmarks. The three micro
benchmarks are built identically; the realistic one replays the recorded flow.

```sh
cargo bench --bench book
```

Medians from CI on a GitHub-hosted `ubuntu-latest` runner. The Go column is from
matching-engine's own CI run (`docs/benchmarks.md`, same runner type, different day).
Shared runners are noisy, so read the ratios as orders of magnitude.

| Benchmark | Rust (this crate) | Go (matching-engine) |
|---|---|---|
| Add and cancel in a 1,000-level book | **94 ns** | 119 ns |
| Take the top of the book (IOC trade) | **20 ns** | 49 ns |
| Rest 40 orders on 10 levels, then sweep them | **2.2 µs** | 5.0 µs |
| Realistic flow, per command | **68 ns** (the recorded 50k flow, with STP/post-only/FOK) | 136 ns (its own 500k recording, a simpler mix) |
| Allocations in steady state | 0 | 0 |

Where the difference comes from:
- **No garbage collector and no write barriers.** Updating links between nodes is a
  plain store.
- **Smaller, contiguous nodes.** `u32` links in one arena, instead of 8-byte pointers to
  separately allocated objects.
- **A cheaper id hash.** One multiply, instead of the Go map's general-purpose hash.

The fuzzer runs about 5,600 command sequences per second. The last CI run executed
671,867 sequences in 2 minutes without a single difference from the reference model.

## API

| | |
|---|---|
| `submit(Order) -> &[Event]` | match a new order; the rest of a GTC limit order rests, the rest of IOC/market is canceled |
| `cancel(id)`, `modify(id, price, qty)` | the same event stream: `Accepted`, `Trade`, `Rested`, `Canceled`, `Modified`, `Rejected` (with a `Reason`) |
| `changes() -> &[LevelChange]` | the levels touched by the last command with their new totals, for incremental market data |
| `best_level`, `depth`, `lookup`, `orders`, `len` | queries |
| `digest()` | hash of the whole state, queue positions included; equal to the Go book's for the same commands |
| `clear()` | empty the book and keep its memory |

The event slices are buffers that the book reuses: each one is valid until the next call,
which the borrow checker enforces.

## Repository layout

```
src/book.rs        the book: arena of orders and levels, matching, queries
src/types.rs       orders, events, validation (enum values match the Go version)
src/reference.rs   the naive reference model, the command decoder, the differential check
src/flow.rs        reader of recorded flows (OBFLOW01) written by matching-engine's cmd/flowdump
src/hash.rs        FNV-1a (digest, conformance) and the order-id hasher
tests/             conformance, property tests, rules, allocations
fuzz/              cargo-fuzz target
benches/           criterion benchmarks
testdata/          the recorded flow (50,000 commands, 1.5 MB)
docs/adr/          design decisions
```

## License

MIT
