use std::panic::{catch_unwind, AssertUnwindSafe};

use nova_nbf::decode;
use nova_query::parse;

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn deterministic_malformed_nbf_inputs_never_panic() {
    let mut state = 0x4e4f_5641_4442_u64;
    for length in 0..=256 {
        let bytes: Vec<u8> = (0..length)
            .map(|_| next(&mut state).to_le_bytes()[0])
            .collect();
        let outcome = catch_unwind(AssertUnwindSafe(|| decode(&bytes)));
        assert!(outcome.is_ok(), "decoder panicked for length {length}");
    }
}

#[test]
fn deterministic_malformed_novaql_inputs_never_panic() {
    const ALPHABET: &[char] = &[
        'a', '0', ' ', '{', '}', '[', ']', '"', '\\', '.', '|', '=', '!', '/', '\n', 'λ',
    ];
    let mut state = 0x4e4f_5641_514c_u64;
    for length in 0..=256 {
        let source: String = (0..length)
            .map(|_| {
                let index = usize::try_from(next(&mut state)).unwrap_or(0) % ALPHABET.len();
                ALPHABET[index]
            })
            .collect();
        let outcome = catch_unwind(AssertUnwindSafe(|| parse(&source)));
        assert!(outcome.is_ok(), "parser panicked for length {length}");
    }
}
