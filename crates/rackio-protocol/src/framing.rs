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
    use tokio::io::{AsyncWriteExt, duplex};

    use crate::{
        MAX_FRAME_BYTES,
        v1::{DiskMetric, MetricSample},
    };

    use super::{FrameError, read_frame, write_frame};

    #[tokio::test]
    async fn round_trips_a_large_but_valid_sample() {
        let (mut client, mut server) = duplex(MAX_FRAME_BYTES * 2);
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

        write_frame(&mut client, &sample)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let decoded: MetricSample = read_frame(&mut server)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded.disks.len(), 5_000);
    }

    #[tokio::test]
    async fn rejects_an_allocation_beyond_the_frame_boundary() {
        let (mut client, mut server) = duplex(16);
        client
            .write_u32(u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or_else(|error| panic!("{error}")))
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(matches!(
            read_frame::<_, MetricSample>(&mut server).await,
            Err(FrameError::TooLarge { .. })
        ));
    }
}
