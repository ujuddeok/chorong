#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_investigation::Investigation;

fuzz_target!(|s: &str| {
    if let Ok(inv) = Investigation::parse(s) {
        assert!(inv.validate().is_ok());
        let json = inv.to_json();
        let back = Investigation::parse(&json)
            .unwrap_or_else(|e| panic!("to_json output failed to re-parse: {e}"));
        assert_eq!(back, inv);
    }
});
