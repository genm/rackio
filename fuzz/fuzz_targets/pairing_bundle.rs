#![no_main]

//! A pairing bundle is the only untrusted text a not-yet-authorised peer can
//! hand this daemon: it arrives by QR scan, clipboard paste or CLI argument
//! before any identity has been verified. Decoding it must therefore reject
//! arbitrary bytes without panicking, over-allocating or looping.

use libfuzzer_sys::fuzz_target;
use rackio_iroh::PairingBundle;

fuzz_target!(|data: &[u8]| {
    let Ok(encoded) = std::str::from_utf8(data) else {
        return;
    };

    // The prefixed form is what a real scan produces. Feeding the raw input too
    // keeps the prefix check itself reachable for the fuzzer.
    for candidate in [encoded, &format!("rackio-pair:{encoded}")] {
        if let Ok(bundle) = PairingBundle::decode(candidate) {
            // Address parsing is the second attacker-reachable step: a bundle
            // that decodes still carries unvalidated endpoint and relay text.
            let _ = bundle.endpoint_addr();
        }
    }
});
