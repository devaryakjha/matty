//! The session token, kept in the system's credential store: the macOS
//! Keychain, the Windows Credential Manager, or the Secret Service on Linux
//! (PLAN §6.6). Nothing under Matty's data folder holds it.
//!
//! The store can be missing, as on a Linux desktop without a Secret
//! Service; then Matty signs in each launch, as before.

use crate::settings::SavedAccount;

/// The credential store's service name for Matty's items.
const SERVICE: &str = "Matty";

/// One item per server and user.
fn entry(account: &SavedAccount) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, &format!("{} {}", account.server, account.user_id))
        .inspect_err(|error| tracing::info!(%error, "no credential store"))
        .ok()
}

/// Keeps `token` for `account`, replacing any older one.
pub fn save(account: &SavedAccount, token: &str) {
    if let Some(entry) = entry(account)
        && let Err(error) = entry.set_password(token)
    {
        tracing::warn!(%error, "could not save the session");
    }
}

/// The token saved for `account`, if there is one.
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
pub fn delete(account: &SavedAccount) {
    match entry(account).map(|entry| entry.delete_credential()) {
        None | Some(Ok(()) | Err(keyring::Error::NoEntry)) => {}
        Some(Err(error)) => tracing::warn!(%error, "could not remove the saved session"),
    }
}
