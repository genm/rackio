//! Shared fixtures for the `remote` module's tests.

use std::net::SocketAddr;

use iroh::SecretKey;
use rackio_core::{NodeInfo, ProtocolVersion};
use uuid::Uuid;

use super::registry::RemoteMachineRecord;

pub(super) fn record() -> RemoteMachineRecord {
    RemoteMachineRecord {
        node: NodeInfo {
            node_id: Uuid::new_v4(),
            display_name: String::from("Test server"),
            os: String::from("linux"),
            architecture: String::from("x86_64"),
            agent_version: String::from("0.1.0"),
            protocol: ProtocolVersion::V1,
            capabilities: Vec::new(),
        },
        endpoint_id: SecretKey::generate().public().to_string(),
        direct_addresses: vec![
            "127.0.0.1:49100"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        ],
        relay_urls: Vec::new(),
        paired_at_ms: 1,
        last_snapshot: None,
    }
}

pub(super) fn address(value: &str) -> SocketAddr {
    value
        .parse()
        .unwrap_or_else(|error| panic!("{value} is not a socket address: {error}"))
}

/// Collect the tracing output of one call, so a test can assert on the
/// event an operator actually reads rather than only on the field it left
/// behind in memory.
pub(super) fn captured_logs(body: impl FnOnce()) -> String {
    #[derive(Clone, Default)]
    struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            let mut buffer = self
                .0
                .lock()
                .map_err(|_| std::io::Error::other("log buffer was poisoned"))?;
            buffer.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let buffer = Buffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, body);
    let bytes = buffer
        .0
        .lock()
        .unwrap_or_else(|error| panic!("{error}"))
        .clone();
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("{error}"))
}
