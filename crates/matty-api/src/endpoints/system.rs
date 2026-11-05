//! `system/ping`, `config/client` and `license/client`.

use crate::{
    ApiError, Client,
    capabilities::{OldFormat, Ping, ServerCapabilities},
};

/// Where [`Client::probe`] stopped.
#[derive(Debug)]
pub enum ProbeError {
    /// `system/ping` failed. A server error or a body that is not the
    /// ping's JSON means something other than Mattermost answered.
    Ping(ApiError),
    /// `system/ping` answered with a status other than `OK`.
    Unhealthy(String),
    /// Ping was fine but the client config or license did not come back.
    Config(ApiError),
}

fn old() -> [(&'static str, String); 1] {
    [("format", "old".to_string())]
}

impl Client {
    /// `POST roles/names`: the roles named, with their permissions, for
    /// gating actions the way the server will (#208).
    pub async fn roles(&self, names: &[String]) -> Result<Vec<crate::models::Role>, ApiError> {
        self.post("roles/names", names).await
    }

    /// `GET system/ping`: whether a Mattermost server answers at this URL.
    pub async fn ping(&self) -> Result<Ping, ApiError> {
        self.get("system/ping").await
    }

    /// `GET config/client?format=old`.
    pub async fn client_config(&self) -> Result<OldFormat, ApiError> {
        self.get_query("config/client", &old()).await
    }

    /// `GET license/client?format=old`.
    pub async fn client_license(&self) -> Result<OldFormat, ApiError> {
        self.get_query("license/client", &old()).await
    }

    /// Checks that a Mattermost server answers here, then reads what it
    /// offers: `system/ping` first, so an address that is not Mattermost
    /// is told apart from a Mattermost server whose config failed.
    pub async fn probe(&self) -> Result<ServerCapabilities, ProbeError> {
        let ping = self.ping().await.map_err(ProbeError::Ping)?;
        if !ping.is_ok() {
            return Err(ProbeError::Unhealthy(ping.status));
        }
        self.capabilities().await.map_err(ProbeError::Config)
    }

    /// Reads the client config and license and derives the feature gates.
    pub async fn capabilities(&self) -> Result<ServerCapabilities, ApiError> {
        let (config, license) = tokio::try_join!(self.client_config(), self.client_license())?;
        ServerCapabilities::from_old_format(&config, &license).map_err(ApiError::Decode)
    }
}
