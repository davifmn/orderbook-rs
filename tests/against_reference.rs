//! Property test: random command sequences on the book and on the naive reference model
//! must give identical events and identical resting orders, and keep the book's internal
//! invariants. The fuzzer (fuzz/) runs the same check on inputs it evolves.

use orderbook::reference::{check_against_reference, decode};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn the_book_matches_the_reference_model(data in proptest::collection::vec(any::<u8>(), 0..3_000)) {
        check_against_reference(&decode(&data));
    }
}

/// A few fixed seeds, so a plain `cargo test` covers long sequences too.
#[test]
fn long_sequences_match_the_reference_model() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..20 {
        let data: Vec<u8> = (0..40_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        check_against_reference(&decode(&data));
    }
}
