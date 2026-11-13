use prost::Message;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// A single v1 frame contains one sample or control message. History is streamed
// as multiple frames, so this protects allocation without truncating a valid response.
pub const MAX_FRAME_BYTES: usize = 1_048_576;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("stream ended before a frame was available")]
    EndOfStream,
    #[error("frame length {actual} exceeds the {maximum}-byte protocol boundary")]
    TooLarge { actual: usize, maximum: usize },
    #[error("I/O failed while transferring a frame: {0}")]
    Io(#[from] std::io::Error),
    #[error("protobuf frame could not be decoded: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("protobuf frame could not be encoded: {0}")]
    Encode(#[from] prost::EncodeError),
}

pub async fn write_frame<W, M>(writer: &mut W, message: &M) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    M: Message,
{
    let encoded_len = message.encoded_len();
    if encoded_len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            actual: encoded_len,
            maximum: MAX_FRAME_BYTES,
        });
    }
    let frame_len = u32::try_from(encoded_len).map_err(|_| FrameError::TooLarge {
        actual: encoded_len,
        maximum: MAX_FRAME_BYTES,
    })?;
    writer.write_u32(frame_len).await?;
    let mut buffer = Vec::with_capacity(encoded_len);
    message.encode(&mut buffer)?;
    writer.write_all(&buffer).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R, M>(reader: &mut R) -> Result<M, FrameError>
where
    R: AsyncRead + Unpin,
    M: Message + Default,
{
    let frame_len = match reader.read_u32().await {
        Ok(value) => value as usize,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::EndOfStream);
        }
        Err(error) => return Err(error.into()),
    };
    if frame_len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            actual: frame_len,
            maximum: MAX_FRAME_BYTES,
        });
    }
    let mut buffer = vec![0_u8; frame_len];
    reader.read_exact(&mut buffer).await?;
    Ok(M::decode(buffer.as_slice())?)
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };

    use prost::Message as _;
    use tokio::io::{AsyncWriteExt, ReadBuf};

    use crate::{
        MAX_FRAME_BYTES,
        v1::{DiskMetric, MetricSample},
    };

    use super::{AsyncRead, FrameError, read_frame, write_frame};

    /// Build a sample whose protobuf encoding is exactly `target` bytes, by
    /// padding a mount path. A large step can widen the two nested length
    /// prefixes and add more than the characters written, so the loop leaves a
    /// margin and closes the last few bytes one character at a time, where the
    /// prefix widths can no longer change.
    fn sample_encoding_to(target: usize) -> MetricSample {
        const PREFIX_MARGIN: usize = 16;

        let mut sample = MetricSample {
            timestamp_ms: 1,
            sequence: 2,
            disks: vec![DiskMetric {
                mount: String::new(),
                total_bytes: 100,
                used_bytes: 50,
            }],
            ..Default::default()
        };
        loop {
            let encoded_len = sample.encoded_len();
            assert!(
                encoded_len <= target,
                "padding overshot the target: {encoded_len} > {target}"
            );
            if encoded_len == target {
                return sample;
            }
            let deficit = target - encoded_len;
            let step = deficit.saturating_sub(PREFIX_MARGIN).max(1);
            match sample.disks.first_mut() {
                Some(disk) => disk.mount.push_str(&"x".repeat(step)),
                None => panic!("the sample always carries one disk"),
            }
        }
    }

    /// A reader whose failure is not an end of stream. `read_frame` must
    /// distinguish the two: a closed connection is a normal end, whereas a
    /// reset one is an I/O error the caller has to see.
    struct FailingReader(io::ErrorKind);

    impl AsyncRead for FailingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(self.0, "synthetic transport failure")))
        }
    }

    #[tokio::test]
    async fn round_trips_a_large_but_valid_sample() {
        // Written into a buffer rather than a live duplex so that a write which
        // silently does nothing fails this test instead of hanging the reader.
        let mut wire = Vec::new();
        let sample = MetricSample {
            timestamp_ms: 1,
            sequence: 2,
            disks: (0..5_000)
                .map(|index| DiskMetric {
                    mount: format!("/volume/{index}"),
                    total_bytes: 100,
                    used_bytes: 50,
                })
                .collect(),
            ..Default::default()
        };

        write_frame(&mut wire, &sample)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!wire.is_empty(), "write_frame produced no bytes");

        let decoded: MetricSample = read_frame(&mut wire.as_slice())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded.disks.len(), 5_000);
    }

    #[tokio::test]
    async fn round_trips_a_frame_of_exactly_the_boundary_size() {
        // The boundary itself is valid on both sides. Without this, a check
        // widened from `>` to `>=` would reject a legitimate maximum frame and
        // no test would notice.
        let sample = sample_encoding_to(MAX_FRAME_BYTES);
        assert_eq!(sample.encoded_len(), MAX_FRAME_BYTES);

        let mut wire = Vec::new();
        write_frame(&mut wire, &sample)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let decoded: MetricSample = read_frame(&mut wire.as_slice())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded.encoded_len(), MAX_FRAME_BYTES);
    }

    #[tokio::test]
    async fn refuses_to_write_one_byte_past_the_boundary() {
        let sample = sample_encoding_to(MAX_FRAME_BYTES + 1);
        let mut wire = Vec::new();

        match write_frame(&mut wire, &sample).await {
            Err(FrameError::TooLarge { actual, maximum }) => {
                assert_eq!(actual, MAX_FRAME_BYTES + 1);
                assert_eq!(maximum, MAX_FRAME_BYTES);
            }
            other => panic!("expected a TooLarge rejection, got {other:?}"),
        }
        assert!(
            wire.is_empty(),
            "an over-sized frame must not reach the transport at all"
        );
    }

    #[tokio::test]
    async fn rejects_an_allocation_beyond_the_frame_boundary() {
        let mut wire = Vec::new();
        wire.write_u32(
            u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        assert!(matches!(
            read_frame::<_, MetricSample>(&mut wire.as_slice()).await,
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn reports_a_closed_stream_as_an_end_of_stream() {
        let empty: &[u8] = &[];
        assert!(matches!(
            read_frame::<_, MetricSample>(&mut { empty }).await,
            Err(FrameError::EndOfStream)
        ));
    }

    #[tokio::test]
    async fn reports_a_truncated_length_prefix_as_an_end_of_stream() {
        // Fewer bytes than the four-byte prefix needs, then a close.
        let truncated: &[u8] = &[0x00, 0x00];
        assert!(matches!(
            read_frame::<_, MetricSample>(&mut { truncated }).await,
            Err(FrameError::EndOfStream)
        ));
    }

    #[tokio::test]
    async fn reports_a_transport_failure_as_an_i_o_error() {
        // A reset connection must not be reported as an orderly end of stream:
        // the caller reconnects on one and stops on the other.
        let mut reader = FailingReader(io::ErrorKind::ConnectionReset);
        assert!(matches!(
            read_frame::<_, MetricSample>(&mut reader).await,
            Err(FrameError::Io(error)) if error.kind() == io::ErrorKind::ConnectionReset
        ));
    }
}
