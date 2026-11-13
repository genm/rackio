#![no_main]

//! `read_frame` reads a length prefix from a peer-controlled stream and then
//! allocates that many bytes. The `MAX_FRAME_BYTES` boundary is the only thing
//! standing between a paired-but-hostile peer and an unbounded allocation, so
//! the length handling is fuzzed against arbitrary stream bytes rather than
//! only the two hand-written boundary tests.

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use rackio_protocol::{read_frame, v1::MetricSample};
use tokio::runtime::Runtime;

// One runtime for the whole process: building it per iteration would dominate
// the run and cut the number of inputs the fuzzer can explore.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime is always constructible")
});

fuzz_target!(|data: &[u8]| {
    RUNTIME.block_on(async {
        let mut stream = data;
        // A single stream may legitimately carry several frames, so keep
        // reading until the decoder rejects the input or the bytes run out.
        while read_frame::<_, MetricSample>(&mut stream).await.is_ok() {}
    });
});
