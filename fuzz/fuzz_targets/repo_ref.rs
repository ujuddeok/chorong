#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_core::RepoRef;

fuzz_target!(|s: &str| {
    if let Ok(r) = RepoRef::parse(s) {
        let shown = r.to_string();
        assert_eq!(RepoRef::parse(&shown), Ok(r), "display {shown:?}");
    }
});
