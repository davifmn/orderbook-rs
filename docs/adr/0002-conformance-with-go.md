# ADR 0002: the Go implementation is the specification

**Status:** accepted

## Context

This crate ports the book of matching-engine (Go). A port is only useful if it behaves
exactly the same, including the corners: self-trade prevention in each mode, fill-or-kill
decisions that depend on self-trade prevention, post-only orders, modifies that keep or
lose queue priority, and the order of the events.

Unit tests ported by hand catch the rules someone thought of. They do not catch a
difference nobody thought of.

## Decision

Three independent checks:

1. **Conformance with the Go book on a recorded flow.** matching-engine's `cmd/flowdump`
   writes 50,000 commands of its realistic order flow, with accounts, all three self-trade
   prevention modes, post-only and fill-or-kill orders added. Next to them it writes what
   the Go book produced:
   - the number of events and trades;
   - an FNV-1a hash over every field of every event;
   - checkpoints of that hash every 1,000 commands;
   - the final `Digest` of the book.

   `tests/conformance.rs` replays the commands and must match all of them. The checkpoints
   locate a divergence within 1,000 commands. The enums use the Go numbering, and
   `Book::digest` hashes the same fields in the same order, so the two digests are directly
   comparable.
2. **Property tests against a naive model.** `reference::RefBook` is a port of the Go
   reference model: one unsorted vector, a linear scan for the best order, and fill-or-kill
   decided by trying the match on a copy. `proptest` generates command sequences over a
   narrow price band with few ids and owners, so crosses, duplicates and self-trades are
   frequent. Events, resting orders and internal invariants must match after every command.
3. **Coverage-guided fuzzing.** The same check under `cargo-fuzz`, two minutes per CI run.

The recorded flow is in the repository (`testdata/flow-50k.bin`, 1.5 MB). To record a new
one, run `go run ./cmd/flowdump -n 50000 -seed 42 -o flow.bin` in matching-engine.

## Consequences

- A change of behaviour in either implementation fails a test in this one. That is the
  point: a behaviour change must be made in both, and the flow re-recorded.
- The flow covers what the generator produces. The rejection paths it never takes
  (invalid ids, prices and quantities, unknown orders) are covered by `tests/rules.rs` and
  by the property tests, whose decoder produces them on purpose.
- Some Go rejections cannot happen here. An invalid side, order type or self-trade
  prevention mode is unrepresentable in the Rust types, so those checks disappear rather
  than being tested.
