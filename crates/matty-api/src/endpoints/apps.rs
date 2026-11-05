//! The Apps framework (#226), served by the `com.mattermost.apps` plugin.

use reqwest::Method;

use crate::{
    ApiError, Client,
    models::{AppBinding, AppCallRequest, AppCallResponse},
};

/// The Apps plugin's API, under `plugins/`.
const APPS: &str = "com.mattermost.apps/api/v1";

/// The `user_agent` Matty sends with bindings and calls.
pub const USER_AGENT: &str = "desktop";

impl Client {
    /// `GET plugins/com.mattermost.apps/api/v1/bindings`: what installed
    /// Apps add in this channel. A server without the Apps plugin answers
    /// 404 ([`ApiError::is_not_found`]).
    pub async fn app_bindings(
        &self,
        channel_id: &str,
        team_id: &str,
    ) -> Result<Vec<AppBinding>, ApiError> {
        let list: Option<Vec<AppBinding>> = self
            .plugin_json(
                Method::GET,
                &format!("{APPS}/bindings"),
                &[
                    ("channel_id", channel_id.to_owned()),
                    ("team_id", team_id.to_owned()),
                    ("user_agent", USER_AGENT.to_owned()),
                ],
                None::<&()>,
            )
            .await?;
        Ok(list.unwrap_or_default())
    }

    /// `POST plugins/com.mattermost.apps/api/v1/call`. An App's `error`
    /// answer comes back as a response, not an [`ApiError`]: the plugin
    /// sends it with a 4xx status and the answer as the body.
    pub async fn app_call(&self, call: &AppCallRequest) -> Result<AppCallResponse, ApiError> {
        match self
            .plugin_json::<_, AppCallResponse>(
                Method::POST,
                &format!("{APPS}/call"),
                &[],
                Some(call),
            )
            .await
        {
            Err(ApiError::Server {
                message, status, ..
            }) if status != 404 => Ok(AppCallResponse {
                kind: crate::models::AppResponseKind::Error,
                text: message,
                ..AppCallResponse::default()
            }),
            other => other,
        }
    }
}
