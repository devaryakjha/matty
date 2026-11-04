//! The one rustls configuration Matty uses for HTTPS and the WebSocket: the
//! operating system's certificate verifier (so company roots installed on the
//! machine are trusted, PLAN §4.5) and the aws-lc-rs crypto provider.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;

use crate::ApiError;

/// The TLS client configuration with `alpn` as its ALPN protocols.
pub(crate) fn client_config(alpn: &[&[u8]]) -> Result<ClientConfig, ApiError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|builder| builder.with_platform_verifier())
        .map_err(|e| ApiError::Tls(e.to_string()))?
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(config)
}

/// The TLS configuration for HTTPS to a host other than the server, such as
/// GitHub for the update feed (#180): the same verifier and provider as the
/// REST client.
pub fn https_config() -> Result<ClientConfig, ApiError> {
    client_config(HTTP_ALPN)
}

/// ALPN for the REST client: HTTP/2 when the server offers it.
pub(crate) const HTTP_ALPN: &[&[u8]] = &[b"h2", b"http/1.1"];
/// ALPN for the WebSocket, which upgrades an HTTP/1.1 connection.
pub(crate) const WEBSOCKET_ALPN: &[&[u8]] = &[b"http/1.1"];
