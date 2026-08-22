use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{BindOpts, Connection, PortmapperConfig, presets},
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
    /// The UDP port this endpoint listens on. `None` takes an ephemeral port,
    /// which changes on every restart and strands viewers that hold only the
    /// previous direct addresses. An operator who monitors this machine over a
    /// direct path configures a fixed port so the address survives a restart.
    #[serde(default)]
    pub bind_port: Option<u16>,
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
    #[error("listen port is invalid: {0}")]
    InvalidBindPort(u16),
    #[error("configured listen port {port} could not be bound: {source}")]
    BindPortUnavailable {
        port: u16,
        source: iroh::endpoint::BindError,
    },
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
    let Some(port) = config.bind_port else {
        return Ok(builder.bind().await?);
    };
    if port == 0 {
        // Port 0 is the ephemeral request iroh already makes by default.
        // Accepting it here would persist a configuration that promises a
        // stable address and does not deliver one.
        return Err(TransportError::InvalidBindPort(port));
    }
    let builder = builder
        // A user-defined unspecified bind replaces iroh's own default for that
        // address family, so each family is requested exactly once.
        .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
        .map_err(|_| TransportError::InvalidBindPort(port))?
        // IPv6 stays optional because iroh's own default allows it to fail:
        // a host without IPv6 must still listen on the configured IPv4 port.
        .bind_addr_with_opts(
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
            BindOpts::default().set_is_required(false),
        )
        .map_err(|_| TransportError::InvalidBindPort(port))?;
    // A configured port that is already taken fails closed. Falling back to an
    // ephemeral port would silently reintroduce the address drift the
    // configuration exists to prevent.
    builder
        .bind()
        .await
        .map_err(|source| TransportError::BindPortUnavailable { port, source })
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

/// The peer's direct IP addresses on an established connection.
///
/// Relay paths are excluded: a relay address identifies the relay, not the
/// peer, and persisting it as a direct address would misdescribe how the peer
/// is reached. The result is sorted and deduplicated so an unchanged address
/// set never looks like a change.
#[must_use]
pub fn observed_direct_addresses(connection: &Connection) -> Vec<SocketAddr> {
    let mut addresses: Vec<SocketAddr> = connection
        .paths()
        .iter()
        .filter_map(|path| match path.remote_addr() {
            TransportAddr::Ip(address) => Some(*address),
            _ => None,
        })
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    addresses
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

    /// The peer's direct IP addresses as observed on this connection.
    ///
    /// These come from the authenticated QUIC session with the pinned endpoint
    /// ID, not from any discovery service, so a caller may use them to refresh
    /// an address set that a peer restart made stale.
    #[must_use]
    pub fn observed_direct_addresses(&self) -> Vec<SocketAddr> {
        observed_direct_addresses(&self.connection)
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
        ClientConnection, ConnectionPath, EndpointConfig, SocketAddr, TransportAddr,
        TransportError, bind_endpoint, direct_path, duration_millis, is_private_or_local,
    };
    use iroh::SecretKey;

    /// Ask the OS for a free UDP port and release it, so a test can request a
    /// specific port without hard-coding one that another process may hold.
    fn free_udp_port() -> u16 {
        let socket = std::net::UdpSocket::bind(("127.0.0.1", 0))
            .unwrap_or_else(|error| panic!("no free UDP port: {error}"));
        socket
            .local_addr()
            .unwrap_or_else(|error| panic!("{error}"))
            .port()
    }

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
    async fn a_configured_listen_port_is_kept_across_restarts() {
        // The whole point of the setting: a restarted daemon must reappear on
        // the address its already paired viewers persisted.
        let port = free_udp_port();
        let config = EndpointConfig {
            bind_port: Some(port),
            ..EndpointConfig::default()
        };
        let secret = SecretKey::generate();

        let first = bind_endpoint(secret.clone(), &config)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let before: Vec<u16> = first.addr().ip_addrs().map(SocketAddr::port).collect();
        first.close().await;
        // `close` stops the endpoint's protocol work, but the UDP sockets live
        // until the endpoint itself is dropped. A restarting daemon exits the
        // process and gets this for free; a test in one process has to be
        // explicit or it races itself for the port.
        drop(first);
        let second = bind_endpoint(secret, &config)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let after: Vec<u16> = second.addr().ip_addrs().map(SocketAddr::port).collect();

        assert!(!before.is_empty(), "a bound endpoint must have an address");
        assert!(
            before.iter().all(|bound| *bound == port),
            "the configured port must be the advertised one, got {before:?}"
        );
        assert_eq!(
            before, after,
            "a restart must not move the endpoint to another port"
        );

        second.close().await;
    }

    #[tokio::test]
    async fn a_listen_port_already_in_use_fails_closed() {
        // Falling back to an ephemeral port here would stand the daemon up on
        // an address no viewer knows, which is exactly the failure the setting
        // exists to prevent.
        let port = free_udp_port();
        let config = EndpointConfig {
            bind_port: Some(port),
            ..EndpointConfig::default()
        };
        let holder = bind_endpoint(SecretKey::generate(), &config)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let error = bind_endpoint(SecretKey::generate(), &config)
            .await
            .err()
            .unwrap_or_else(|| panic!("a taken port must not bind a second endpoint"));

        match error {
            TransportError::BindPortUnavailable { port: reported, .. } => {
                assert_eq!(reported, port, "the error must name the configured port");
            }
            other => panic!("a taken port must be reported as such, got {other}"),
        }

        holder.close().await;
    }

    #[tokio::test]
    async fn an_ephemeral_listen_port_cannot_be_configured_as_stable() {
        let error = bind_endpoint(
            SecretKey::generate(),
            &EndpointConfig {
                bind_port: Some(0),
                ..EndpointConfig::default()
            },
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("port 0 promises a stable address it cannot keep"));

        assert!(matches!(error, TransportError::InvalidBindPort(0)));
    }

    #[tokio::test]
    async fn an_established_connection_reports_the_peer_address_it_reached() {
        // A viewer recovers a moved peer by learning the address of the
        // session it already authenticated, so that address has to be readable
        // from the connection itself.
        let port = free_udp_port();
        let server = bind_endpoint(
            SecretKey::generate(),
            &EndpointConfig {
                bind_port: Some(port),
                ..EndpointConfig::default()
            },
        )
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

        let observed = client.observed_direct_addresses();

        assert!(
            observed.iter().all(|address| address.port() == port),
            "an observed direct address must be the peer's own listen address, got {observed:?}"
        );
        assert!(
            !observed.is_empty(),
            "a direct session must expose the address it reached"
        );

        client.close();
        drop(incoming);
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
