//! The Rust book must do exactly what the Go book does.
//!
//! `testdata/flow-50k.bin` was written by matching-engine's `cmd/flowdump`: 50,000
//! commands of a realistic flow (with accounts, all three self-trade prevention modes,
//! post-only and fill-or-kill orders) plus the Go book's results: the number of events and
//! trades, a hash of every event field, checkpoints of that hash every 1,000 commands, and
//! the final digest. Replaying the commands here must reproduce all of them.

use orderbook::flow::Flow;
use orderbook::hash::Fnv1a;
use orderbook::{Book, EventKind};

#[test]
fn replaying_the_go_flow_gives_the_same_events_and_the_same_book() {
    let flow = Flow::parse(include_bytes!("../testdata/flow-50k.bin")).expect("valid flow file");
    let expected = &flow.expected;
    assert_eq!(flow.commands.len(), 50_000);

    let mut book = Book::new();
    let mut hash = Fnv1a::default();
    let (mut events, mut trades) = (0u64, 0u64);
    for (i, cmd) in flow.commands.iter().enumerate() {
        for e in cmd.apply(&mut book) {
            for f in e.fields() {
                hash.put(f);
            }
            events += 1;
            if e.kind == EventKind::Trade {
                trades += 1;
            }
        }
        book.changes();
        if (i + 1) % expected.checkpoint_every == 0 {
            let k = (i + 1) / expected.checkpoint_every - 1;
            assert_eq!(
                hash.sum(),
                expected.checkpoints[k],
                "first difference with the Go book between commands {} and {}",
                i + 1 - expected.checkpoint_every,
                i
            );
        }
    }

    assert_eq!((events, trades), (expected.events, expected.trades));
    assert_eq!(hash.sum(), expected.event_hash, "event hash");
    assert_eq!(book.digest(), expected.digest, "final book digest");
    book.check_invariants();
}

#[test]
fn a_damaged_flow_file_is_refused() {
    let mut data = include_bytes!("../testdata/flow-50k.bin").to_vec();
    data.truncate(data.len() - 3);
    assert!(Flow::parse(&data).is_err());
    assert!(Flow::parse(b"NOTAFLOW").is_err());
}
