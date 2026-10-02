#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| ck::fuzzing::route(data));
