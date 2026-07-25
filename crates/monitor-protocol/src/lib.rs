mod framing;

pub mod v1 {
    #![allow(clippy::doc_markdown, clippy::must_use_candidate)]
    include!(concat!(env!("OUT_DIR"), "/tray_monitor.v1.rs"));
}

pub use framing::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};

pub const ALPN: &[u8] = b"tray-monitor/metrics/1";
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 0;

#[must_use]
pub const fn current_version() -> v1::ProtocolVersion {
    v1::ProtocolVersion {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    }
}

#[must_use]
pub const fn compatible(version: &v1::ProtocolVersion) -> bool {
    version.major == PROTOCOL_MAJOR
}
