//! The servers the user signed in to (#206, PLAN §3 M2, §6.5). Each saved
//! server is one [`SavedAccount`] in [`Settings::servers`], in rail order,
//! and keeps the two names that find its data: `data_key` for its database
//! and media folder, `keychain_key` for its session in the credential store.
//! [`Settings::account`] is the one shown now; with none, launch opens the
//! first.
//!
//! Matty kept a single `account` before; [`Settings::migrate_servers`] makes
//! it the first saved server with the keys it already used, so its sign-in
//! and cache carry over. An older Matty reading the file still finds
//! `account` and ignores the rest.

use gpui_kit::Context;

use crate::settings::{SavedAccount, Settings, SettingsStore};

impl SavedAccount {
    /// Fills the keys a server saved before #206 lacks, from the address
    /// and user, exactly as that Matty named its files.
    fn fill_keys(&mut self) {
        if !matty_sync::is_account_key(&self.data_key) {
            self.data_key = derived_data_key(&self.server, &self.user_id);
        }
        if self.keychain_key.is_empty() {
            self.keychain_key = format!("{} {}", self.server, self.user_id);
        }
    }

    /// The key naming this account's database and media folder. A saved
    /// key a hand edit spoiled is ignored for the one Matty would make.
    pub fn data_key(&self) -> String {
        if matty_sync::is_account_key(&self.data_key) {
            self.data_key.clone()
        } else {
            derived_data_key(&self.server, &self.user_id)
        }
    }

    /// The name of this account's item in the credential store.
    pub fn keychain_key(&self) -> String {
        if self.keychain_key.is_empty() {
            format!("{} {}", self.server, self.user_id)
        } else {
            self.keychain_key.clone()
        }
    }

    /// What the rail and its tooltip call the server.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            crate::live::url_host(&self.server)
        } else {
            &self.name
        }
    }
}

/// The data key for `user_id` on `server`; empty for an address that does
/// not parse, which names no file.
fn derived_data_key(server: &str, user_id: &str) -> String {
    url::Url::parse(server)
        .map(|server| matty_sync::account_key(&server, user_id))
        .unwrap_or_default()
}

impl Settings {
    /// Keeps the saved servers whole: each has its keys, each appears once,
    /// and the current account is one of them, its fields the newest. Runs
    /// on every load and change, so a file from before #206 gains its one
    /// server here.
    pub(crate) fn migrate_servers(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.servers.retain_mut(|server| {
            server.fill_keys();
            seen.insert(server.data_key.clone())
        });
        for index in 0..self.servers.len() {
            if self.servers[index].name.is_empty() {
                self.servers[index].name = self.server_name(&self.servers[index].server);
            }
        }
        let Some(mut account) = self.account.take() else {
            return;
        };
        account.fill_keys();
        let saved = self
            .servers
            .iter()
            .position(|server| server.data_key == account.data_key);
        // An older Matty may have changed the account without knowing the
        // server's name.
        if account.name.is_empty() {
            account.name = match saved {
                Some(index) => self.servers[index].name.clone(),
                None => self.server_name(&account.server),
            };
        }
        match saved {
            Some(index) => self.servers[index] = account.clone(),
            None => self.servers.insert(0, account.clone()),
        }
        self.account = Some(account);
    }

    /// What a server check called `url`, or else its host.
    fn server_name(&self, url: &str) -> String {
        self.recent_servers
            .iter()
            .find(|recent| recent.url.eq_ignore_ascii_case(url))
            .map_or_else(
                || crate::live::url_host(url).to_owned(),
                |recent| recent.name.clone(),
            )
    }

    /// Saves `account` after it signed in and makes it current. A server
    /// signed in to before keeps its place on the rail; a new one goes last,
    /// named as its server check found it.
    pub fn sign_in_server(&mut self, mut account: SavedAccount) {
        account.fill_keys();
        let saved = self
            .servers
            .iter()
            .position(|server| server.data_key == account.data_key);
        if account.name.is_empty() {
            account.name = match saved {
                Some(index) => self.servers[index].name.clone(),
                None => self.server_name(&account.server),
            };
        }
        match saved {
            Some(index) => self.servers[index] = account.clone(),
            None => self.servers.push(account.clone()),
        }
        self.account = Some(account);
    }

    /// Forgets the server saved under `data_key`, returning it so its data
    /// can be deleted. If it was current, none is until another is chosen.
    pub fn remove_server(&mut self, data_key: &str) -> Option<SavedAccount> {
        let index = self
            .servers
            .iter()
            .position(|server| server.data_key == data_key)?;
        let removed = self.servers.remove(index);
        if self
            .account
            .as_ref()
            .is_some_and(|account| account.data_key() == data_key)
        {
            self.account = None;
        }
        Some(removed)
    }

    /// Makes the `index`th saved server current, returning it.
    pub fn select_server(&mut self, index: usize) -> Option<SavedAccount> {
        let server = self.servers.get(index)?.clone();
        self.account = Some(server.clone());
        Some(server)
    }

    /// Moves the saved server at `from` to `to`, counted once it has left
    /// `from`, as the rail's drag reports it. The current server stays
    /// current wherever it goes.
    pub fn move_server(&mut self, from: usize, to: usize) {
        if from >= self.servers.len() || to >= self.servers.len() {
            return;
        }
        let server = self.servers.remove(from);
        self.servers.insert(to, server);
    }

    /// Where the current server sits on the rail.
    pub fn current_server(&self) -> Option<usize> {
        let current = self.account.as_ref()?.data_key();
        self.servers
            .iter()
            .position(|server| server.data_key == current)
    }
}

impl SettingsStore {
    /// The server to open at launch: the current one, or else the first
    /// saved, which becomes current, as after the current one was removed.
    pub fn launch_server(&mut self, cx: &mut Context<Self>) -> Option<SavedAccount> {
        if self.get().account.is_none() && !self.get().servers.is_empty() {
            self.update(cx, |settings| {
                settings.select_server(0);
            });
        }
        self.get().account.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RecentServer;

    fn account(server: &str, user_id: &str) -> SavedAccount {
        SavedAccount {
            server: server.into(),
            user_id: user_id.into(),
            username: user_id.into(),
            display_name: user_id.to_uppercase(),
            ..SavedAccount::default()
        }
    }

    /// A settings file from before #206: one account and no servers.
    const SINGLE_ACCOUNT: &str = r#"{
        "zoom": 1.0,
        "account": {
            "server": "https://chat.example.com:8065",
            "user_id": "u1",
            "username": "arya",
            "display_name": "Arya"
        },
        "recent_servers": [{"url": "https://chat.example.com:8065", "name": "Example"}]
    }"#;

    #[test]
    fn a_single_account_becomes_the_first_server_with_its_old_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, SINGLE_ACCOUNT).unwrap();
        let (settings, _) = crate::settings::load(&path);
        let current = settings.account.clone().unwrap();
        assert_eq!(settings.servers, std::slice::from_ref(&current));
        assert_eq!(settings.current_server(), Some(0));
        // The cache and the session are found where they always were.
        assert_eq!(current.data_key, "chat.example.com_8065-u1");
        assert_eq!(current.keychain_key, "https://chat.example.com:8065 u1");
        let server = url::Url::parse("https://chat.example.com:8065").unwrap();
        assert_eq!(
            Some(crate::live::database_path(&current)),
            matty_sync::database_path(&server, "u1")
        );
        assert_eq!(
            crate::live::media_dir(&current),
            matty_sync::media_dir(&server, "u1")
        );
        assert_eq!(current.label(), "Example");

        // Saved again, it reads back the same, and an older Matty still
        // finds its `account`.
        crate::settings::save(&path, &settings).unwrap();
        let (again, _) = crate::settings::load(&path);
        assert_eq!(again, settings);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["account"]["user_id"], "u1");
    }

    #[test]
    fn signing_in_adds_servers_in_order_and_again_keeps_the_place() {
        let mut settings = Settings {
            recent_servers: vec![RecentServer {
                url: "https://b.example.com".into(),
                name: "Bee".into(),
            }],
            ..Settings::default()
        };
        settings.sign_in_server(account("https://a.example.com", "u1"));
        settings.sign_in_server(account("https://b.example.com", "u2"));
        let names: Vec<_> = settings.servers.iter().map(|s| s.label()).collect();
        assert_eq!(names, ["a.example.com", "Bee"]);
        assert_eq!(settings.current_server(), Some(1));

        let mut renamed = account("https://a.example.com", "u1");
        renamed.display_name = "Renamed".into();
        settings.sign_in_server(renamed);
        assert_eq!(settings.servers.len(), 2);
        assert_eq!(settings.current_server(), Some(0));
        assert_eq!(settings.servers[0].display_name, "Renamed");
        assert_eq!(settings.servers[0].label(), "a.example.com");
    }

    #[test]
    fn a_change_to_the_current_account_reaches_its_server() {
        let mut settings = Settings::default();
        settings.sign_in_server(account("https://a.example.com", "u1"));
        settings.sign_in_server(account("https://b.example.com", "u2"));
        if let Some(current) = &mut settings.account {
            current.display_name = "New name".into();
        }
        settings.migrate_servers();
        assert_eq!(settings.servers[1].display_name, "New name");
        assert_eq!(settings.servers.len(), 2);
    }

    #[test]
    fn removing_the_current_server_leaves_the_next_to_launch() {
        let mut settings = Settings::default();
        settings.sign_in_server(account("https://a.example.com", "u1"));
        settings.sign_in_server(account("https://b.example.com", "u2"));
        let key = settings.account.as_ref().unwrap().data_key();
        let removed = settings.remove_server(&key).unwrap();
        assert_eq!(removed.user_id, "u2");
        assert_eq!(settings.account, None);
        assert_eq!(settings.remove_server(&key), None);
        assert_eq!(settings.select_server(0).unwrap().user_id, "u1");
        assert_eq!(settings.select_server(3), None);
    }

    #[test]
    fn a_moved_server_keeps_its_place_and_the_current_one_stays() {
        let mut settings = Settings::default();
        settings.sign_in_server(account("https://a.example.com", "u1"));
        settings.sign_in_server(account("https://b.example.com", "u2"));
        settings.sign_in_server(account("https://c.example.com", "u3"));
        settings.move_server(2, 0);
        let order: Vec<_> = settings
            .servers
            .iter()
            .map(|s| s.user_id.as_str())
            .collect();
        assert_eq!(order, ["u3", "u1", "u2"]);
        assert_eq!(settings.current_server(), Some(0));
        settings.move_server(0, 3);
        assert_eq!(
            settings.servers[0].user_id, "u3",
            "out of range moves nothing"
        );
    }

    #[test]
    fn a_spoiled_or_repeated_server_is_mended() {
        let mut spoiled = account("https://a.example.com", "u1");
        spoiled.data_key = "../../home".into();
        let mut settings = Settings {
            servers: vec![spoiled, account("https://a.example.com", "u1")],
            ..Settings::default()
        };
        settings.migrate_servers();
        assert_eq!(settings.servers.len(), 1);
        assert_eq!(settings.servers[0].data_key, "a.example.com-u1");
    }

    #[gpui_kit::test]
    fn launch_opens_the_first_server_when_none_is_current(cx: &mut gpui_kit::TestAppContext) {
        let store = cx.update(|cx| SettingsStore::init(None, cx));
        assert_eq!(store.update(cx, |store, cx| store.launch_server(cx)), None);
        store.update(cx, |store, cx| {
            store.update(cx, |s| {
                s.sign_in_server(account("https://a.example.com", "u1"));
                s.sign_in_server(account("https://b.example.com", "u2"));
                s.account = None;
            })
        });
        let launched = store
            .update(cx, |store, cx| store.launch_server(cx))
            .unwrap();
        assert_eq!(launched.user_id, "u1");
        store.read_with(cx, |store, _| {
            assert_eq!(store.get().current_server(), Some(0))
        });
    }
}
