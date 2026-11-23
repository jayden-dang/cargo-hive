//! Exercises shared production reader decoding and semantic models.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cargo_hive_core::unit_path::fuzzing::artifact_names(data);
    cargo_hive_gc::explain::fuzzing::artifact_names(data);
});
