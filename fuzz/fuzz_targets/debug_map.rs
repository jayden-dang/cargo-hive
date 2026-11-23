//! Exercises shared production reader decoding and semantic models.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cargo_hive_gc::debug_map::fuzzing::debug_map(data);
});
