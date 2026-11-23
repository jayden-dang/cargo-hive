//! Exercises shared production reader decoding and semantic models.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cargo_hive_core::fingerprint::fuzzing::diagnostics(data);
    cargo_hive_gc::explain::fuzzing::diagnostics(data);
});
