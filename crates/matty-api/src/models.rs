//! Mattermost data models. Fields Matty does not read yet are ignored when
//! decoding, so new server fields never break Matty.

use serde::{Deserialize, Serialize};

/// A user, as `users/me` and `users/ids` return it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct User {
    /// User id.
    pub id: String,
    /// Username, without `@`.
    pub username: String,
    /// Email address, when the server shows it.
    pub email: String,
    /// Given name.
    pub first_name: String,
    /// Family name.
    pub last_name: String,
    /// Nickname.
    pub nickname: String,
    /// Space-separated role names.
    pub roles: String,
    /// The sign-in service (`""` for email, `ldap`, `gitlab`, `saml`, ...).
    pub auth_service: String,
    /// Milliseconds since the epoch; changes when the avatar changes.
    pub last_picture_update: i64,
    /// Milliseconds since the epoch; nonzero when deactivated.
    pub delete_at: i64,
    /// Whether the user is a bot.
    pub is_bot: bool,
    /// The user's locale, such as `en`.
    pub locale: String,
}

/// A session, as `users/{id}/sessions` returns it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Session {
    /// Session id (not the token).
    pub id: String,
    /// The device id the session was created with.
    pub device_id: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch; 0 for no expiry.
    pub expires_at: i64,
    /// Milliseconds since the epoch.
    pub last_activity_at: i64,
}
