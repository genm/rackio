use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{BindOpts, Connection, PortmapperConfig, presets},
    tls::{CaTlsConfig, default_provider},
};
use rackio_core::ConnectionPath;
use rackio_protocol::{
    FrameError,
    v1::{Request, Response},
};
use rustls_pki_types::{CertificateDer, pem::PemObject as _};
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
    /// Addresses this machine is reachable on but cannot observe on any of its
    /// own interfaces, such as the `IP:PORT` a router forwards to it.
    ///
    /// They are handed to the endpoint as external addresses, so they join the
    /// endpoint's own advertised set and become traversal candidates. Nothing
    /// resolves, probes or corrects them: this stays operator-supplied
    /// configuration, and a wrong entry is simply a candidate that never
    /// answers.
    #[serde(default)]
    pub advertise_addresses: Vec<SocketAddr>,
    /// A PEM file holding the certificate authority that signs the configured
    /// relay's TLS certificate.
    ///
    /// `None` — the default — leaves iroh's own `EmbeddedWebPki` anchor in
    /// place, so a relay must present a publicly trusted certificate. `Some`
    /// replaces that root set for relay connections with exactly the
    /// certificates in this file, which is how an organisation reaches a relay
    /// on an internal network whose certificate a public CA would never issue.
    ///
    /// The path is stored as the operator wrote it and read when the endpoint
    /// binds. It only ever applies alongside a configured relay: direct-only
    /// mode builds no relay transport, so there is nothing for a CA to anchor.
    #[serde(default)]
    pub relay_ca_certificate: Option<PathBuf>,
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
    #[error(transparent)]
    RelayCa(#[from] RelayCaError),
}

/// Why an operator-supplied relay CA file cannot become a trust anchor.
///
/// Every variant names the file and the correction to make. None of them has a
/// fallback: a relay whose CA cannot be loaded is refused rather than attempted
/// against the public root store, because quietly widening the trust anchor is
/// the opposite of what pinning was configured to do.
///
/// The file's contents are never included. The certificate is not a secret, but
/// an error is not a place to dump a PEM either.
#[derive(Debug, Error)]
pub enum RelayCaError {
    #[error("relay CA certificate `{path}` could not be read: {source}")]
    Unreadable {
        path: String,
        source: std::io::Error,
    },
    #[error("relay CA certificate `{path}` is not readable PEM: {source}")]
    Malformed {
        path: String,
        source: rustls_pki_types::pem::Error,
    },
    #[error(
        "relay CA certificate `{path}` contains no PEM CERTIFICATE block; export the issuing CA \
         itself in PEM form, not a private key, a request or a DER file"
    )]
    NoCertificate { path: String },
    #[error(
        "relay CA certificate `{path}` holds no usable certificate authority: {source}; check \
         that the file is the CA certificate rather than the relay's own leaf certificate"
    )]
    Unusable {
        path: String,
        source: std::io::Error,
    },
}

/// Check that an operator-supplied relay CA file can actually anchor a relay's
/// TLS certificate, before anything stores it.
///
/// This is the same work `bind_endpoint` does, so a file accepted here is one
/// the next daemon start can use. It touches only the local filesystem: no name
/// is resolved and no relay is contacted.
pub fn validate_relay_ca_certificate(path: &Path) -> Result<(), RelayCaError> {
    relay_ca_tls_config(path).map(|_| ())
}

/// Build the relay TLS trust configuration pinned to `path`.
///
/// `CaTlsConfig::custom_roots` trusts the supplied roots *only*: the public root
/// set is replaced rather than extended, so a pinned relay cannot silently fall
/// back to a publicly issued certificate.
fn relay_ca_tls_config(path: &Path) -> Result<CaTlsConfig, RelayCaError> {
    let display = path.display().to_string();
    let pem = std::fs::read(path).map_err(|source| RelayCaError::Unreadable {
        path: display.clone(),
        source,
    })?;
    let roots = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| RelayCaError::Malformed {
            path: display.clone(),
            source,
        })?;
    // Content that holds no CERTIFICATE section at all — an empty file, a key,
    // or bytes that are not PEM — yields an empty iterator rather than an
    // error, so the emptiness is what has to be rejected.
    if roots.is_empty() {
        return Err(RelayCaError::NoCertificate { path: display });
    }
    let config = CaTlsConfig::custom_roots(roots);
    // Building the verifier now is what makes this a real check. `rustls`
    // *drops* certificates it cannot parse when it fills a root store, so a PEM
    // block whose body is not an X.509 CA would otherwise produce an empty
    // anchor set that only fails at the first relay connection, long after the
    // operator was told the configuration was accepted.
    config
        .server_cert_verifier(default_provider())
        .map_err(|source| RelayCaError::Unusable {
            path: display,
            source,
        })?;
    Ok(config)
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
        // Avoid constructing any relay mode in direct-only runtime. A pinned CA
        // is deliberately not applied here: with no relay transport there is no
        // TLS connection for it to anchor, and installing a trust anchor for a
        // transport that does not exist would only blur what direct-only means.
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
        let builder = builder.relay_mode(RelayMode::Custom(RelayMap::from_iter(urls)));
        match config.relay_ca_certificate.as_deref() {
            // A CA the operator pinned and the daemon cannot load stops the
            // start. Binding anyway would leave the endpoint trusting the
            // public root store for a relay chosen precisely because no public
            // root vouches for it.
            Some(path) => builder.ca_tls_config(relay_ca_tls_config(path)?),
            None => builder,
        }
    };
    // Tell the endpoint about the addresses the operator knows and the host
    // cannot see. `external_addr` publishes them alongside the observed ones
    // and uses them for NAT traversal, which is what turns a forwarded or
    // NAT-mapped address into a real candidate instead of a bundle entry.
    let builder = config
        .advertise_addresses
        .iter()
        .fold(builder, |builder, address| builder.external_addr(*address));
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
        ClientConnection, ConnectionPath, EndpointConfig, RelayCaError, SocketAddr, TransportAddr,
        TransportError, bind_endpoint, direct_path, duration_millis, is_private_or_local,
        validate_relay_ca_certificate,
    };
    use iroh::SecretKey;

    /// A self-signed certificate authority, generated once for these tests and
    /// valid until 2126. It has no private key anywhere: a trust anchor is a
    /// public certificate, and nothing here ever issues from it.
    const TEST_CA_PEM: &str = include_str!("../testdata/relay-ca.pem");

    /// A syntactically perfect PEM section whose body is not an X.509
    /// certificate. `rustls` silently discards such an entry while filling a
    /// root store, so this is the input that proves the check is real.
    const NOT_A_CERTIFICATE_PEM: &str = "\
-----BEGIN CERTIFICATE-----
aGVsbG8gcmFja2lv
-----END CERTIFICATE-----
";

    fn write_pem(directory: &tempfile::TempDir, name: &str, contents: &str) -> std::path::PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, contents).unwrap_or_else(|error| panic!("{error}"));
        path
    }

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
        // Dropping releases the sockets, but the runtime tasks holding them are
        // torn down asynchronously, so the port can still be occupied for a few
        // milliseconds afterwards. A real restart never sees this — the kernel
        // reclaims everything when the process exits — so retry briefly rather
        // than let an in-process teardown race report a product failure. The
        // bound is short and the last error is still surfaced, so a port that
        // genuinely cannot be reclaimed fails the test instead of hanging it.
        let mut second = None;
        for attempt in 0..40 {
            match bind_endpoint(secret.clone(), &config).await {
                Ok(endpoint) => {
                    second = Some(endpoint);
                    break;
                }
                Err(error) => {
                    assert!(
                        attempt < 39,
                        "configured port never became bindable: {error}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        let second = second.unwrap_or_else(|| panic!("configured port never became bindable"));
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
    async fn a_configured_advertised_address_becomes_an_endpoint_address() {
        // An operator behind NAT knows the address their router forwards and
        // the host cannot see it. Unless the endpoint is told, the address is
        // only ever a pairing-bundle entry: it is neither reported back to the
        // operator nor usable to open a path mid-session.
        let advertised: SocketAddr = "198.51.100.7:41641"
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let endpoint = bind_endpoint(
            SecretKey::generate(),
            &EndpointConfig {
                advertise_addresses: vec![advertised],
                ..EndpointConfig::default()
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

        let addresses: Vec<SocketAddr> = endpoint.addr().ip_addrs().copied().collect();

        assert!(
            addresses.contains(&advertised),
            "a configured address must be one the endpoint advertises, got {addresses:?}"
        );
        assert!(
            addresses.len() > 1,
            "a configured address must be added to the observed ones, not replace them"
        );

        endpoint.close().await;
    }

    #[tokio::test]
    async fn an_unreachable_advertised_address_does_not_change_failure_behaviour() {
        // A wrong or stale entry stays an ordinary candidate that never
        // answers. Nothing resolves or probes it, so it must not stop the
        // endpoint from binding or from reaching a peer over a usable address.
        let advertised: SocketAddr = "203.0.113.9:41641"
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let server = bind_endpoint(
            SecretKey::generate(),
            &EndpointConfig {
                advertise_addresses: vec![advertised],
                ..EndpointConfig::default()
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
        let client_endpoint = bind_endpoint(SecretKey::generate(), &EndpointConfig::default())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let server_address = server.addr();
        assert!(
            server_address
                .ip_addrs()
                .any(|address| *address == advertised),
            "the unreachable candidate is offered like any other"
        );
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
        assert!(
            !client.observed_direct_addresses().contains(&advertised),
            "an address that never answered must not be reported as one the session reached"
        );

        client.close();
        drop(incoming);
        client_endpoint.close().await;
        server.close().await;
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

    #[test]
    fn a_certificate_authority_in_pem_form_is_accepted_as_a_relay_anchor() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "ca.pem", TEST_CA_PEM);

        validate_relay_ca_certificate(&path).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn several_authorities_in_one_file_are_all_kept() {
        // An organisation rotating its CA runs both anchors for a while. Taking
        // only the first section would drop the relay the moment it switched to
        // the new certificate.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let bundle = format!("{TEST_CA_PEM}{TEST_CA_PEM}");
        let path = write_pem(&directory, "bundle.pem", &bundle);

        validate_relay_ca_certificate(&path).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn a_missing_relay_ca_file_is_refused_by_name() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("absent.pem");

        let Err(error) = validate_relay_ca_certificate(&path) else {
            panic!("a CA file that does not exist must not be accepted");
        };

        assert!(
            matches!(error, RelayCaError::Unreadable { .. }),
            "got {error}"
        );
        assert!(
            error.to_string().contains("absent.pem"),
            "the operator must be told which path failed: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_relay_ca_file_the_daemon_cannot_read_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "ca.pem", TEST_CA_PEM);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .unwrap_or_else(|error| panic!("{error}"));
        // root ignores the mode, so the case this test describes cannot exist
        // there. Skip rather than assert a failure the OS will not produce.
        if std::fs::read(&path).is_ok() {
            return;
        }

        let Err(error) = validate_relay_ca_certificate(&path) else {
            panic!("an unreadable CA file must not be accepted");
        };

        assert!(
            matches!(error, RelayCaError::Unreadable { .. }),
            "got {error}"
        );
    }

    #[test]
    fn a_file_that_is_not_pem_at_all_is_refused() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "garbage.pem", "this is not a certificate\n");

        let Err(error) = validate_relay_ca_certificate(&path) else {
            panic!("arbitrary bytes must not be accepted as a certificate authority");
        };

        assert!(
            matches!(error, RelayCaError::NoCertificate { .. }),
            "got {error}"
        );
    }

    #[test]
    fn an_empty_relay_ca_file_is_refused() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "empty.pem", "");

        let Err(error) = validate_relay_ca_certificate(&path) else {
            panic!("an empty file carries no trust anchor");
        };

        assert!(
            matches!(error, RelayCaError::NoCertificate { .. }),
            "got {error}"
        );
    }

    #[test]
    fn pem_that_carries_no_certificate_is_refused() {
        // A private key is valid PEM and a plausible mistake — it is the file
        // sitting next to the certificate in every relay deployment.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(
            &directory,
            "key.pem",
            "-----BEGIN PRIVATE KEY-----\naGVsbG8gcmFja2lv\n-----END PRIVATE KEY-----\n",
        );

        let Err(error) = validate_relay_ca_certificate(&path) else {
            panic!("a key is not a trust anchor");
        };

        assert!(
            matches!(error, RelayCaError::NoCertificate { .. }),
            "got {error}"
        );
    }

    #[test]
    fn a_certificate_section_that_is_not_a_certificate_is_refused() {
        // The failure this guards is specific: `rustls` drops unparsable
        // entries instead of complaining, so without an explicit check this
        // file would be stored as valid and leave an empty anchor set that only
        // failed at the first relay connection.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "fake.pem", NOT_A_CERTIFICATE_PEM);

        let Err(error) = validate_relay_ca_certificate(&path) else {
            panic!("a CERTIFICATE header does not make the body a certificate");
        };

        assert!(
            matches!(error, RelayCaError::Unusable { .. }),
            "got {error}"
        );
    }

    #[test]
    fn validating_a_relay_ca_contacts_nothing() {
        // The check is local by construction, and the bound is the evidence: a
        // relay probe or a name lookup for the CA's issuer would not return
        // inside it.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "ca.pem", TEST_CA_PEM);

        let started = std::time::Instant::now();
        validate_relay_ca_certificate(&path).unwrap_or_else(|error| panic!("{error}"));
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "validating a relay CA took {elapsed:?}; it must not touch the network"
        );
    }

    #[tokio::test]
    async fn a_relay_whose_pinned_ca_cannot_be_loaded_does_not_bind() {
        // Binding anyway would stand the daemon up trusting the public root
        // store for a relay that was pinned precisely because no public root
        // vouches for it — an unusable relay must be visibly unusable.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "fake.pem", NOT_A_CERTIFICATE_PEM);

        let error = bind_endpoint(
            SecretKey::generate(),
            &EndpointConfig {
                relay_urls: vec![String::from("https://relay.example.test")],
                relay_ca_certificate: Some(path),
                ..EndpointConfig::default()
            },
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("a relay with an unloadable CA must not start"));

        assert!(
            matches!(
                error,
                TransportError::RelayCa(RelayCaError::Unusable { .. })
            ),
            "got {error}"
        );
    }

    #[tokio::test]
    async fn a_pinned_relay_ca_does_not_reach_the_direct_only_path() {
        // Direct-only mode builds no relay transport at all. A CA left in the
        // configuration must not resurrect one, and must not stop the endpoint
        // from binding either.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = write_pem(&directory, "ca.pem", TEST_CA_PEM);

        let config = EndpointConfig {
            relay_ca_certificate: Some(path),
            ..EndpointConfig::default()
        };
        let server = bind_endpoint(SecretKey::generate(), &config)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let client_endpoint = bind_endpoint(SecretKey::generate(), &config)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(
            server.addr().relay_urls().next().is_none(),
            "a machine with no relay configured must advertise no relay address"
        );

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

        assert_eq!(
            client.details().path,
            ConnectionPath::LanDirect,
            "a pinned CA must not change how a direct session is reached or reported"
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
