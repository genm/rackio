mod framing;

pub mod v1 {
    #![allow(clippy::doc_markdown, clippy::must_use_candidate)]
    include!(concat!(env!("OUT_DIR"), "/rackio.v1.rs"));
}

pub use framing::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};

pub const ALPN: &[u8] = b"rackio/metrics/1";
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 1;

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

#[cfg(test)]
mod tests {
    use super::{PROTOCOL_MAJOR, PROTOCOL_MINOR, compatible, current_version, v1::ProtocolVersion};

    #[test]
    fn accepts_its_own_version() {
        assert!(compatible(&current_version()));
        assert_eq!(current_version().major, PROTOCOL_MAJOR);
        assert_eq!(current_version().minor, PROTOCOL_MINOR);
    }

    #[test]
    fn rejects_every_other_major_version() {
        // The compatibility check is a fail-closed boundary: an unrecognised
        // major must be refused rather than negotiated down, in both directions
        // so that neither an older nor a newer peer is silently admitted.
        for major in [
            PROTOCOL_MAJOR - 1,
            PROTOCOL_MAJOR + 1,
            PROTOCOL_MAJOR + 100,
            u32::MAX,
        ] {
            assert!(
                !compatible(&ProtocolVersion { major, minor: 0 }),
                "major {major} must be rejected"
            );
        }
    }

    #[test]
    fn ignores_the_minor_version_within_a_major() {
        // Minor is additive by definition, so a peer ahead or behind on minor
        // stays compatible. A check widened to compare the whole version would
        // break rolling upgrades.
        for minor in [0, PROTOCOL_MINOR + 1, u32::MAX] {
            assert!(
                compatible(&ProtocolVersion {
                    major: PROTOCOL_MAJOR,
                    minor,
                }),
                "minor {minor} must stay compatible"
            );
        }
    }
}
