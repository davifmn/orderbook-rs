//! Coverage-guided fuzzing: the fuzzer evolves byte strings, which become command
//! sequences; the book and the naive reference model must agree on every one.
#![no_main]

use libfuzzer_sys::fuzz_target;
use orderbook::reference::{check_against_reference, decode};

fuzz_target!(|data: &[u8]| {
    check_against_reference(&decode(data));
});
