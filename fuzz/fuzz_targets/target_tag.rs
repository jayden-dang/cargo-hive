//! Exercises shared production reader decoding and semantic models.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cargo_hive_core::target::fuzzing::target_tag(data);
});
