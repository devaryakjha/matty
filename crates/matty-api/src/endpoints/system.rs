//! `system/ping`, `config/client` and `license/client`.

use crate::{
    ApiError, Client,
    capabilities::{OldFormat, Ping, ServerCapabilities},
};

fn old() -> [(&'static str, String); 1] {
    [("format", "old".to_string())]
}

impl Client {
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

    /// Reads the client config and license and derives the feature gates.
    pub async fn capabilities(&self) -> Result<ServerCapabilities, ApiError> {
        let (config, license) = tokio::try_join!(self.client_config(), self.client_license())?;
        ServerCapabilities::from_old_format(&config, &license).map_err(ApiError::Decode)
    }
}
