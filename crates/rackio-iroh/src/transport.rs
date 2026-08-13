use std::{net::IpAddr, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{Connection, PortmapperConfig, presets},
};
use rackio_core::ConnectionPath;
use rackio_protocol::{
    FrameError,
    v1::{Request, Response},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndpointConfig {
    /// Empty means strict direct-only mode. Vendor relay/discovery defaults are
    /// never inherited.
    pub relay_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionDetails {
    pub path: ConnectionPath,
    pub rtt_ms: u64,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("relay URL is invalid: {0}")]
    InvalidRelayUrl(String),
    #[error("endpoint failed: {0}")]
    Endpoint(#[from] iroh::endpoint::BindError),
    #[error("connection failed: {0}")]
    Connect(#[from] iroh::endpoint::ConnectError),
    #[error("connection stream failed: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error("connection stream was closed: {0}")]
    StreamClosed(#[from] iroh::endpoint::ClosedStream),
    #[error("protocol frame failed: {0}")]
    Frame(#[from] FrameError),
    #[error("remote request failed with {code}: {message}")]
    Remote { code: String, message: String },
}

pub async fn bind_endpoint(
    secret_key: SecretKey,
    config: &EndpointConfig,
) -> Result<Endpoint, TransportError> {
    // Minimal installs only a crypto provider. In particular, it does not add
    // iroh's vendor DNS discovery or public relay map.
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![rackio_protocol::ALPN.to_vec()])
        .portmapper_config(PortmapperConfig::Disabled);
    let builder = if config.relay_urls.is_empty() {
        // Avoid constructing any relay mode in direct-only runtime.
        builder.clear_relay_transports()
    } else {
        let urls = config
            .relay_urls
            .iter()
            .map(|value| {
                value
                    .parse::<RelayUrl>()
                    .map_err(|_| TransportError::InvalidRelayUrl(value.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        builder.relay_mode(RelayMode::Custom(RelayMap::from_iter(urls)))
    };
    Ok(builder.bind().await?)
}

#[must_use]
pub fn classify_connection(connection: &Connection) -> ConnectionDetails {
    let paths = connection.paths();
    let selected = paths.iter().find(iroh::endpoint::Path::is_selected);
    let Some(selected) = selected else {
        return ConnectionDetails {
            path: ConnectionPath::Unknown,
            rtt_ms: 0,
        };
    };
    let path = if selected.is_relay() {
        // Relay is decided by the transport, never inferred from the address:
        // reporting a relayed path as direct would misstate who can observe
        // the connection's metadata.
        ConnectionPath::Relayed
    } else {
        direct_path(selected.remote_addr())
    };
    ConnectionDetails {
        path,
        rtt_ms: duration_millis(selected.rtt()),
    }
}

/// Classify a non-relayed path from the peer address it reaches.
///
/// Only an IP transport is direct. Anything else is reported as unknown rather
/// than optimistically as direct.
fn direct_path(address: &TransportAddr) -> ConnectionPath {
    match address {
        TransportAddr::Ip(address) if is_private_or_local(address.ip()) => {
            ConnectionPath::LanDirect
        }
        TransportAddr::Ip(_) => ConnectionPath::WanDirect,
        _ => ConnectionPath::Unknown,
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn is_private_or_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_private_or_local_v4(address),
        // An IPv4-mapped peer address describes the same IPv4 host, so classify
        // it as IPv4 rather than reporting a LAN peer as `wan_direct`.
        IpAddr::V6(address) => address.to_ipv4_mapped().map_or_else(
            || {
                address.is_loopback()
                    || address.is_unique_local()
                    || address.is_unicast_link_local()
            },
            is_private_or_local_v4,
        ),
    }
}

fn is_private_or_local_v4(address: std::net::Ipv4Addr) -> bool {
    address.is_private() || address.is_loopback() || address.is_link_local()
}

#[derive(Debug)]
pub struct ClientConnection {
    // Keep the shared endpoint alive without owning its daemon-wide shutdown.
    _endpoint: Endpoint,
    local_id: EndpointId,
    connection: Connection,
}

impl ClientConnection {
    pub async fn connect(
        endpoint: Endpoint,
        address: EndpointAddr,
    ) -> Result<Self, TransportError> {
        let local_id = endpoint.id();
        let connection = endpoint.connect(address, rackio_protocol::ALPN).await?;
        Ok(Self {
            _endpoint: endpoint,
            local_id,
            connection,
        })
    }

    #[must_use]
    pub fn remote_id(&self) -> EndpointId {
        self.connection.remote_id()
    }

    #[must_use]
    pub fn local_id(&self) -> EndpointId {
        self.local_id
    }

    #[must_use]
    pub fn details(&self) -> ConnectionDetails {
        classify_connection(&self.connection)
    }

    pub async fn request(&self, request: &Request) -> Result<Response, TransportError> {
        let mut responses = self.stream(request).await?;
        responses.next().await
    }

    pub async fn stream(&self, request: &Request) -> Result<ResponseStream, TransportError> {
        let (mut send, receive) = self.connection.open_bi().await?;
        rackio_protocol::write_frame(&mut send, request).await?;
        send.finish()?;
        Ok(ResponseStream { receive })
    }

    pub fn close(self) {
        self.connection.close(0_u32.into(), b"client shutdown");
    }
}

#[derive(Debug)]
pub struct ResponseStream {
    receive: iroh::endpoint::RecvStream,
}

impl ResponseStream {
    pub async fn next(&mut self) -> Result<Response, TransportError> {
        Ok(rackio_protocol::read_frame(&mut self.receive).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ClientConnection, ConnectionPath, EndpointConfig, TransportAddr, bind_endpoint,
        direct_path, duration_millis, is_private_or_local,
    };
    use iroh::SecretKey;

    fn address(value: &str) -> std::net::IpAddr {
        value
            .parse()
            .unwrap_or_else(|error| panic!("{value} is not an address: {error}"))
    }

    #[test]
    fn ipv4_mapped_lan_addresses_are_not_classified_as_wan() {
        assert!(is_private_or_local(address("::ffff:192.168.1.5")));
        assert!(is_private_or_local(address("::ffff:127.0.0.1")));
        assert!(!is_private_or_local(address("::ffff:93.184.216.34")));
        assert!(is_private_or_local(address("192.168.1.5")));
        assert!(is_private_or_local(address("fe80::1")));
        assert!(!is_private_or_local(address("2001:db8::1")));
    }

    #[test]
    fn every_local_ipv6_form_is_recognised_on_its_own() {
        // Each of these is local for a different reason, so they have to be
        // accepted independently rather than only in combination.
        assert!(is_private_or_local(address("::1")), "loopback");
        assert!(is_private_or_local(address("fd00::1")), "unique local");
        assert!(is_private_or_local(address("fe80::1")), "link local");
    }

    #[test]
    fn a_direct_path_is_classified_by_the_address_it_reaches() {
        fn socket(value: &str) -> TransportAddr {
            TransportAddr::Ip(
                value
                    .parse()
                    .unwrap_or_else(|error| panic!("{value} is not a socket address: {error}")),
            )
        }

        assert_eq!(
            direct_path(&socket("192.168.1.5:7777")),
            ConnectionPath::LanDirect
        );
        assert_eq!(
            direct_path(&socket("[::ffff:10.0.0.2]:7777")),
            ConnectionPath::LanDirect
        );
        assert_eq!(
            direct_path(&socket("93.184.216.34:7777")),
            ConnectionPath::WanDirect,
            "a public peer must not be reported as a LAN peer"
        );
        assert_eq!(
            direct_path(&socket("[2001:db8::1]:7777")),
            ConnectionPath::WanDirect
        );
    }

    #[test]
    fn a_round_trip_time_is_reported_in_whole_milliseconds() {
        assert_eq!(duration_millis(Duration::from_micros(1_500)), 1);
        assert_eq!(duration_millis(Duration::from_millis(1_500)), 1_500);
        assert_eq!(
            duration_millis(Duration::MAX),
            u64::MAX,
            "an unrepresentable duration saturates rather than wrapping to a plausible RTT"
        );
    }

    #[tokio::test]
    async fn closing_a_client_connection_is_observed_by_the_peer() {
        // A viewer that stops monitoring must release the agent's side too. A
        // close that never reaches the peer leaves the agent serving a session
        // nobody is reading until its own idle timeout expires.
        let server = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let client_endpoint = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let server_address = server.addr();
        let accept = tokio::spawn({
            let server = server.clone();
            async move {
                server
                    .accept()
                    .await
                    .unwrap_or_else(|| panic!("endpoint closed"))
                    .await
                    .unwrap_or_else(|error| panic!("{error}"))
            }
        });
        let client = ClientConnection::connect(client_endpoint.clone(), server_address)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let incoming = accept.await.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(client.remote_id(), server.id());
        assert_eq!(client.local_id(), client_endpoint.id());

        client.close();

        let closed = tokio::time::timeout(Duration::from_secs(10), incoming.closed())
            .await
            .unwrap_or_else(|_| {
                panic!("the peer must be told, not left waiting for its own timeout")
            });
        match closed {
            iroh::endpoint::ConnectionError::ApplicationClosed(close) => {
                // Dropping the connection would also close it, but silently.
                // The reason is what tells the agent's log that the viewer
                // left deliberately rather than lost its network.
                assert_eq!(
                    close.reason.as_ref(),
                    b"client shutdown",
                    "the peer must learn why the session ended"
                );
            }
            other => panic!("a deliberate close must be an application close, got {other}"),
        }

        client_endpoint.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn direct_only_endpoint_connects_without_vendor_discovery() {
        let server = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let client = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let server_address = server.addr();
        let accept = tokio::spawn({
            let server = server.clone();
            async move {
                server
                    .accept()
                    .await
                    .unwrap_or_else(|| panic!("endpoint closed"))
                    .await
                    .unwrap_or_else(|error| panic!("{error}"))
            }
        });
        let outgoing = client
            .connect(server_address, rackio_protocol::ALPN)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let incoming = accept.await.unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(outgoing.remote_id(), server.id());
        assert_eq!(incoming.remote_id(), client.id());
        assert!(outgoing.paths().iter().any(|path| path.is_ip()));

        outgoing.close(0_u32.into(), b"test complete");
        client.close().await;
        server.close().await;
    }
}
