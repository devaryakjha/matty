//! Sign-in, sessions and the current user (PLAN §6.3 "Login details", §6.6).

use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

use crate::{
    ApiError, Client,
    models::{Session, User},
};

/// The error id Mattermost uses when an MFA code is required or wrong. Matty
/// detects MFA by this id, not by the HTTP status.
pub const MFA_REQUIRED_ID: &str = "mfa.validate_token.authenticate.app_error";

/// What to sign in with.
#[derive(Clone, Default)]
pub struct Credentials {
    /// Email or username.
    pub login_id: String,
    /// Password.
    pub password: String,
    /// The MFA code, once the server has asked for one.
    pub mfa_token: Option<String>,
    /// Sign in against AD/LDAP only.
    pub ldap_only: bool,
    /// A push-notification device, `<platform>:<token>` such as
    /// `apple:…` or `android_rn:…`. Leave it empty on desktop: servers from
    /// 12.0 refuse the sign-in when it is anything else.
    pub device_id: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("login_id", &self.login_id)
            .field("password", &"[redacted]")
            .field("mfa_token", &self.mfa_token.as_ref().map(|_| "[redacted]"))
            .field("ldap_only", &self.ldap_only)
            .field("device_id", &self.device_id)
            .finish()
    }
}

#[derive(Serialize)]
struct LoginBody<'a> {
    login_id: &'a str,
    password: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    token: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    device_id: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    ldap_only: bool,
}

/// A successful sign-in: a client that carries the new session, and the user.
#[derive(Debug)]
pub struct SignedIn {
    /// The same server, now with the session token.
    pub client: Client,
    /// The signed-in user.
    pub user: User,
}

impl Client {
    /// `POST users/login`. The session token comes back in the `Token` header.
    /// Returns [`ApiError::MfaRequired`] when the account needs an MFA code
    /// (call again with `mfa_token` set).
    pub async fn login(&self, credentials: &Credentials) -> Result<SignedIn, ApiError> {
        let body = LoginBody {
            login_id: &credentials.login_id,
            password: &credentials.password,
            token: credentials.mfa_token.as_deref().unwrap_or(""),
            device_id: &credentials.device_id,
            ldap_only: credentials.ldap_only,
        };
        let body = serde_json::to_vec(&body).map_err(|e| ApiError::Decode(e.to_string()))?;
        self.sign_in("users/login", body).await
    }

    /// `POST users/login/desktop_token`: finishes SSO with the short-lived
    /// `server_token` from the `mattermost://` hand-off (PLAN §6.6).
    pub async fn login_desktop_token(
        &self,
        server_token: &str,
        device_id: &str,
    ) -> Result<SignedIn, ApiError> {
        let body = serde_json::json!({ "token": server_token, "device_id": device_id });
        self.sign_in("users/login/desktop_token", body.to_string().into_bytes())
            .await
    }

    /// Signs in with a personal access token: `with_token` plus `users/me`.
    pub async fn login_personal_access_token(&self, token: &str) -> Result<SignedIn, ApiError> {
        let client = self.with_token(token);
        let user = client.me().await?;
        Ok(SignedIn { client, user })
    }

    async fn sign_in(&self, path: &str, body: Vec<u8>) -> Result<SignedIn, ApiError> {
        let response = self
            .send(Method::POST, path, &[], Some(body))
            .await
            .map_err(mfa)?;
        let token = response
            .headers()
            .get("token")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ApiError::Decode("the server did not return a session token".into()))?;
        let bytes = response.bytes().await.map_err(ApiError::from_transport)?;
        let user: User =
            serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok(SignedIn {
            client: self.with_token(token),
            user,
        })
    }

    /// `POST users/logout`: ends this client's session on the server.
    pub async fn logout(&self) -> Result<(), ApiError> {
        self.post::<_, Value>("users/logout", &serde_json::json!({}))
            .await
            .map(|_| ())
    }

    /// `GET users/me`.
    pub async fn me(&self) -> Result<User, ApiError> {
        self.get("users/me").await
    }

    /// `GET users/{user_id}/sessions`.
    pub async fn sessions(&self, user_id: &str) -> Result<Vec<Session>, ApiError> {
        self.get(&format!("users/{user_id}/sessions")).await
    }

    /// `POST users/{user_id}/sessions/revoke`.
    pub async fn revoke_session(&self, user_id: &str, session_id: &str) -> Result<(), ApiError> {
        self.post::<_, Value>(
            &format!("users/{user_id}/sessions/revoke"),
            &serde_json::json!({ "session_id": session_id }),
        )
        .await
        .map(|_| ())
    }
}

fn mfa(error: ApiError) -> ApiError {
    if error.id() == Some(MFA_REQUIRED_ID) {
        ApiError::MfaRequired
    } else {
        error
    }
}
