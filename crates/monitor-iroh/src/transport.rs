use std::{net::IpAddr, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{Connection, PortmapperConfig, presets},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tray_monitor_core::ConnectionPath;
use tray_monitor_protocol::{
    FrameError,
    v1::{Request, Response},
};

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
}

pub async fn bind_endpoint(
    secret_key: SecretKey,
    config: &EndpointConfig,
) -> Result<Endpoint, TransportError> {
    let relay_mode = if config.relay_urls.is_empty() {
        RelayMode::Disabled
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
        RelayMode::custom(urls)
    };

    // Minimal installs only a crypto provider. In particular, it does not add
    // iroh's vendor DNS discovery or public relay map.
    Ok(Endpoint::builder(presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![tray_monitor_protocol::ALPN.to_vec()])
        .relay_mode(relay_mode)
        .portmapper_config(PortmapperConfig::Disabled)
        .bind()
        .await?)
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
        ConnectionPath::Relayed
    } else if selected.is_ip() {
        match selected.remote_addr() {
            TransportAddr::Ip(address) if is_private_or_local(address.ip()) => {
                ConnectionPath::LanDirect
            }
            TransportAddr::Ip(_) => ConnectionPath::WanDirect,
            _ => ConnectionPath::Unknown,
        }
    } else {
        ConnectionPath::Unknown
    };
    ConnectionDetails {
        path,
        rtt_ms: duration_millis(selected.rtt()),
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn is_private_or_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() || address.is_loopback() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    }
}

#[derive(Debug)]
pub struct ClientConnection {
    endpoint: Endpoint,
    connection: Connection,
}

impl ClientConnection {
    pub async fn connect(
        endpoint: Endpoint,
        address: EndpointAddr,
    ) -> Result<Self, TransportError> {
        let connection = endpoint
            .connect(address, tray_monitor_protocol::ALPN)
            .await?;
        Ok(Self {
            endpoint,
            connection,
        })
    }

    #[must_use]
    pub fn remote_id(&self) -> EndpointId {
        self.connection.remote_id()
    }

    #[must_use]
    pub fn local_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    #[must_use]
    pub fn details(&self) -> ConnectionDetails {
        classify_connection(&self.connection)
    }

    pub async fn request(&self, request: &Request) -> Result<Response, TransportError> {
        let (mut send, mut receive) = self.connection.open_bi().await?;
        tray_monitor_protocol::write_frame(&mut send, request).await?;
        send.finish()?;
        Ok(tray_monitor_protocol::read_frame(&mut receive).await?)
    }

    pub async fn close(self) {
        self.connection.close(0_u32.into(), b"client shutdown");
        self.endpoint.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{EndpointConfig, bind_endpoint};
    use iroh::SecretKey;

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
            .connect(server_address, tray_monitor_protocol::ALPN)
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
