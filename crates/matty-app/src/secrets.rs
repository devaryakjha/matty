//! The session token, kept in the system's credential store: the macOS
//! Keychain, the Windows Credential Manager, or the Secret Service on Linux
//! (PLAN §6.6). Nothing under Matty's data folder holds it.
//!
//! The store can be missing, as on a Linux desktop without a Secret
//! Service; then Matty signs in each launch, as before. On the web the
//! token is kept in the page's `localStorage`.

use crate::settings::SavedAccount;

/// The credential store's service name for Matty's items.
const SERVICE: &str = "Matty";

/// One item per server and user.
#[cfg(not(target_family = "wasm"))]
fn entry(account: &SavedAccount) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, &account.keychain_key())
        .inspect_err(|error| tracing::info!(%error, "no credential store"))
        .ok()
}

/// Keeps `token` for `account`, replacing any older one.
#[cfg(not(target_family = "wasm"))]
pub fn save(account: &SavedAccount, token: &str) {
    if let Some(entry) = entry(account)
        && let Err(error) = entry.set_password(token)
    {
        tracing::warn!(%error, "could not save the session");
    }
}

/// The token saved for `account`, if there is one.
#[cfg(not(target_family = "wasm"))]
pub fn load(account: &SavedAccount) -> Option<String> {
    match entry(account)?.get_password() {
        Ok(token) if !token.is_empty() => Some(token),
        Ok(_) | Err(keyring::Error::NoEntry) => None,
        Err(error) => {
            tracing::warn!(%error, "could not read the saved session");
            None
        }
    }
}

/// Forgets `account`'s token, on sign-out or when it expired.
#[cfg(not(target_family = "wasm"))]
pub fn delete(account: &SavedAccount) {
    match entry(account).map(|entry| entry.delete_credential()) {
        None | Some(Ok(()) | Err(keyring::Error::NoEntry)) => {}
        Some(Err(error)) => tracing::warn!(%error, "could not remove the saved session"),
    }
}

/// The `localStorage` key for `account`'s token on the web.
#[cfg(target_family = "wasm")]
fn key(account: &SavedAccount) -> String {
    format!("{SERVICE} session {}", account.keychain_key())
}

/// Keeps `token` for `account`. The browser has no credential store, so on
/// the web it goes in this origin's `localStorage`.
#[cfg(target_family = "wasm")]
pub fn save(account: &SavedAccount, token: &str) {
    if let Err(error) = crate::web_storage::set(&key(account), token) {
        tracing::warn!(%error, "could not save the session");
    }
}

/// The token saved for `account`, if there is one.
#[cfg(target_family = "wasm")]
pub fn load(account: &SavedAccount) -> Option<String> {
    crate::web_storage::get(&key(account))
        .ok()
        .filter(|token| !token.is_empty())
}

/// Forgets `account`'s token, on sign-out or when it expired.
#[cfg(target_family = "wasm")]
pub fn delete(account: &SavedAccount) {
    let _ = crate::web_storage::remove(&key(account));
}
