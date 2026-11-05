//! What a server can do, from `config/client` and `license/client`
//! (`format=old`, where every value is a string). Matty feature-gates on this
//! at connect time (PLAN §6.3).

use std::{collections::HashMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A Mattermost server version, `major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl Version {
    /// Builds a version from its parts.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = String;

    /// Accepts `10.5.0`, `10.5` and dev builds such as `10.5.0-rc1` or
    /// `10.5.0.12345.abc`, reading the first three numbers.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parts = text
            .trim()
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|part| part.parse::<u32>());
        let mut next = |required: bool| match parts.next() {
            Some(Ok(number)) => Ok(number),
            None | Some(Err(_)) if !required => Ok(0),
            _ => Err(format!("`{text}` is not a version")),
        };
        Ok(Self::new(next(true)?, next(true)?, next(false)?))
    }
}

/// The oldest server Matty supports.
///
/// `POST /users/login/desktop_token`, which SSO needs, arrived in 9.1.0
/// (spike S3, `docs/spikes/S3.md`). PLAN §6.3 supports only recent
/// extended-support releases, so the floor is 9.5.0, the first of them with
/// the desktop-token flow.
pub const MINIMUM_VERSION: Version = Version::new(9, 5, 0);

/// How Collapsed Reply Threads is configured on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapsedThreads {
    /// Turned off; threads are always expanded.
    Disabled,
    /// Available, off unless a user turns it on.
    DefaultOff,
    /// Available, on unless a user turns it off.
    DefaultOn,
    /// Always on.
    AlwaysOn,
}

impl CollapsedThreads {
    /// Whether a user can have threads collapsed at all.
    pub fn available(self) -> bool {
        self != Self::Disabled
    }
}

/// SSO providers the server has turned on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SsoProviders {
    /// GitLab.
    pub gitlab: bool,
    /// Google.
    pub google: bool,
    /// Office 365 (Entra ID).
    pub office365: bool,
    /// OpenID Connect.
    pub openid: bool,
    /// SAML 2.0.
    pub saml: bool,
}

impl SsoProviders {
    /// Whether any SSO provider is on.
    pub fn any(self) -> bool {
        self.gitlab || self.google || self.office365 || self.openid || self.saml
    }
}

/// The server version is older than Matty supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("This server runs Mattermost {found}; Matty needs {minimum} or later.")]
pub struct UnsupportedVersion {
    /// The server's version.
    pub found: Version,
    /// The oldest version Matty supports.
    pub minimum: Version,
}

/// The feature gates Matty reads from a server.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerCapabilities {
    /// Server version.
    pub version: Version,
    /// Server build number.
    pub build_number: Option<String>,
    /// The site name the admin set.
    pub site_name: Option<String>,
    /// AD/LDAP sign-in is on.
    pub ldap: bool,
    /// The label the server gives LDAP sign-in, such as "AD/LDAP".
    pub ldap_login_label: Option<String>,
    /// Sign-in with an email address is allowed.
    pub email_login: bool,
    /// Sign-in with a username is allowed.
    pub username_login: bool,
    /// Enabled SSO providers.
    pub sso: SsoProviders,
    /// Collapsed Reply Threads mode.
    pub collapsed_threads: CollapsedThreads,
    /// Custom emoji are on.
    pub custom_emoji: bool,
    /// Link previews are on.
    pub link_previews: bool,
    /// Drafts sync through the server.
    pub synced_drafts: bool,
    /// Message priority is on.
    pub post_priority: bool,
    /// The longest message the server accepts, in characters.
    pub max_post_size: usize,
    /// The largest upload the server accepts, in bytes.
    pub max_file_size: u64,
    /// Whether images go through the server's image proxy.
    pub image_proxy: bool,
    /// Channel bookmarks are on (#226): the `ChannelBookmarks` feature flag
    /// where the server still has it (9.x), and every server from 10.0 on,
    /// which dropped the flag once bookmarks shipped.
    pub channel_bookmarks: bool,
    /// The server has a license.
    pub licensed: bool,
    /// The license tier, such as `professional` or `enterprise`.
    pub sku: Option<String>,
}

/// The raw `format=old` payload: every value is a string.
pub type OldFormat = HashMap<String, serde_json::Value>;

fn text<'a>(map: &'a OldFormat, key: &str) -> Option<&'a str> {
    map.get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
}

fn flag(map: &OldFormat, key: &str) -> bool {
    match map.get(key) {
        Some(serde_json::Value::String(value)) => value.eq_ignore_ascii_case("true"),
        Some(serde_json::Value::Bool(value)) => *value,
        _ => false,
    }
}

fn number(map: &OldFormat, key: &str) -> Option<u64> {
    match map.get(key)? {
        serde_json::Value::String(value) => value.trim().parse().ok(),
        serde_json::Value::Number(value) => value.as_u64(),
        _ => None,
    }
}

/// Mattermost's default `MaxPostSize` when the server does not send one.
const DEFAULT_MAX_POST_SIZE: usize = 16_383;
/// Mattermost's default `MaxFileSize` (100 MB).
const DEFAULT_MAX_FILE_SIZE: u64 = 104_857_600;

impl ServerCapabilities {
    /// Reads the client config and license, both requested with `format=old`.
    pub fn from_old_format(config: &OldFormat, license: &OldFormat) -> Result<Self, String> {
        let version = text(config, "Version")
            .ok_or("the server did not report its version")?
            .parse::<Version>()?;
        let licensed = flag(license, "IsLicensed");
        let collapsed_threads = match text(config, "CollapsedThreads") {
            Some("default_off") => CollapsedThreads::DefaultOff,
            Some("default_on") => CollapsedThreads::DefaultOn,
            Some("always_on") => CollapsedThreads::AlwaysOn,
            _ => CollapsedThreads::Disabled,
        };
        Ok(Self {
            version,
            build_number: text(config, "BuildNumber").map(str::to_string),
            site_name: text(config, "SiteName").map(str::to_string),
            ldap: flag(config, "EnableLdap"),
            ldap_login_label: text(config, "LdapLoginFieldName").map(str::to_string),
            email_login: flag(config, "EnableSignInWithEmail"),
            username_login: flag(config, "EnableSignInWithUsername"),
            sso: SsoProviders {
                gitlab: flag(config, "EnableSignUpWithGitLab"),
                google: flag(config, "EnableSignUpWithGoogle"),
                office365: flag(config, "EnableSignUpWithOffice365"),
                openid: flag(config, "EnableSignUpWithOpenId"),
                saml: flag(config, "EnableSaml"),
            },
            collapsed_threads,
            custom_emoji: flag(config, "EnableCustomEmoji"),
            link_previews: flag(config, "EnableLinkPreviews"),
            synced_drafts: flag(config, "AllowSyncedDrafts"),
            post_priority: flag(config, "PostPriority"),
            max_post_size: number(config, "MaxPostSize")
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(DEFAULT_MAX_POST_SIZE),
            max_file_size: number(config, "MaxFileSize").unwrap_or(DEFAULT_MAX_FILE_SIZE),
            image_proxy: flag(config, "HasImageProxy"),
            channel_bookmarks: match config.get("FeatureFlagChannelBookmarks") {
                Some(_) => flag(config, "FeatureFlagChannelBookmarks"),
                None => version >= Version::new(10, 0, 0),
            },
            licensed,
            sku: licensed
                .then(|| text(license, "SkuShortName").map(str::to_string))
                .flatten(),
        })
    }

    /// Fails when the server is older than [`MINIMUM_VERSION`].
    pub fn minimum_supported(&self) -> Result<(), UnsupportedVersion> {
        if self.version < MINIMUM_VERSION {
            Err(UnsupportedVersion {
                found: self.version,
                minimum: MINIMUM_VERSION,
            })
        } else {
            Ok(())
        }
    }
}

/// The `system/ping` answer.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Ping {
    /// `OK` when healthy.
    pub status: String,
}

impl Ping {
    /// Whether the server reported itself healthy.
    pub fn is_ok(&self) -> bool {
        self.status.eq_ignore_ascii_case("ok")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> OldFormat {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
    }

    fn team() -> ServerCapabilities {
        ServerCapabilities::from_old_format(
            &load("config_client_team.json"),
            &load("license_client_unlicensed.json"),
        )
        .unwrap()
    }

    fn enterprise() -> ServerCapabilities {
        ServerCapabilities::from_old_format(
            &load("config_client_enterprise.json"),
            &load("license_client_enterprise.json"),
        )
        .unwrap()
    }

    #[test]
    fn parses_versions() {
        assert_eq!("10.5.0".parse(), Ok(Version::new(10, 5, 0)));
        assert_eq!("9.11".parse(), Ok(Version::new(9, 11, 0)));
        assert_eq!("10.5.1-rc2".parse(), Ok(Version::new(10, 5, 1)));
        assert_eq!("10.5.0.13001.abcdef".parse(), Ok(Version::new(10, 5, 0)));
        assert!("dev".parse::<Version>().is_err());
        assert!(Version::new(9, 11, 0) > Version::new(9, 5, 3));
    }

    #[test]
    fn team_edition_fixture_decodes() {
        let caps = team();
        assert_eq!(caps.version, Version::new(10, 5, 0));
        assert!(!caps.licensed);
        assert_eq!(caps.sku, None);
        assert!(caps.email_login && caps.username_login);
        assert!(!caps.ldap);
        assert_eq!(
            caps.sso,
            SsoProviders {
                gitlab: true,
                ..Default::default()
            }
        );
        assert_eq!(caps.collapsed_threads, CollapsedThreads::DefaultOn);
        assert_eq!(caps.max_post_size, 16_383);
        assert_eq!(caps.max_file_size, 104_857_600);
    }

    #[test]
    fn enterprise_fixture_decodes() {
        let caps = enterprise();
        assert!(caps.licensed);
        assert_eq!(caps.sku.as_deref(), Some("enterprise"));
        assert!(caps.ldap);
        assert_eq!(caps.ldap_login_label.as_deref(), Some("AD/LDAP"));
        assert!(caps.sso.saml && caps.sso.openid && caps.sso.office365);
        assert_eq!(caps.collapsed_threads, CollapsedThreads::AlwaysOn);
        assert!(caps.post_priority);
        assert!(caps.image_proxy);
    }

    #[test]
    fn each_gate_turns_on_and_off() {
        let base = load("config_client_team.json");
        let license = load("license_client_unlicensed.json");
        let gate = |key: &str, value: &str| {
            let mut config = base.clone();
            config.insert(key.into(), value.into());
            ServerCapabilities::from_old_format(&config, &license).unwrap()
        };
        type Gate = fn(&ServerCapabilities) -> bool;
        let cases: [(&str, Gate); 11] = [
            ("EnableLdap", |c| c.ldap),
            ("EnableSignUpWithGitLab", |c| c.sso.gitlab),
            ("EnableSignUpWithGoogle", |c| c.sso.google),
            ("EnableSignUpWithOffice365", |c| c.sso.office365),
            ("EnableSignUpWithOpenId", |c| c.sso.openid),
            ("EnableSaml", |c| c.sso.saml),
            ("EnableCustomEmoji", |c| c.custom_emoji),
            ("EnableLinkPreviews", |c| c.link_previews),
            ("AllowSyncedDrafts", |c| c.synced_drafts),
            ("PostPriority", |c| c.post_priority),
            ("HasImageProxy", |c| c.image_proxy),
        ];
        for (key, read) in cases {
            assert!(read(&gate(key, "true")), "{key} on");
            assert!(!read(&gate(key, "false")), "{key} off");
        }
        assert!(
            !gate("CollapsedThreads", "disabled")
                .collapsed_threads
                .available()
        );
        assert!(
            gate("CollapsedThreads", "default_off")
                .collapsed_threads
                .available()
        );
        assert_eq!(gate("MaxPostSize", "4000").max_post_size, 4000);
        assert!(gate("FeatureFlagChannelBookmarks", "true").channel_bookmarks);
        assert!(!gate("FeatureFlagChannelBookmarks", "false").channel_bookmarks);
        assert_eq!(gate("MaxFileSize", "52428800").max_file_size, 52_428_800);
    }

    #[test]
    fn bookmarks_follow_the_flag_or_the_version() {
        let license = load("license_client_unlicensed.json");
        let caps = |version: &str, flag: Option<&str>| {
            let mut config = load("config_client_team.json");
            config.insert("Version".into(), version.into());
            config.remove("FeatureFlagChannelBookmarks");
            if let Some(flag) = flag {
                config.insert("FeatureFlagChannelBookmarks".into(), flag.into());
            }
            ServerCapabilities::from_old_format(&config, &license)
                .unwrap()
                .channel_bookmarks
        };
        assert!(!caps("9.11.0", None));
        assert!(caps("9.11.0", Some("true")));
        assert!(caps("12.0.1", None));
        assert!(!caps("10.5.0", Some("false")));
    }

    #[test]
    fn rejects_servers_below_the_floor() {
        let mut caps = team();
        assert_eq!(caps.minimum_supported(), Ok(()));
        caps.version = Version::new(9, 4, 9);
        let error = caps.minimum_supported().unwrap_err();
        assert_eq!(error.found, Version::new(9, 4, 9));
        assert_eq!(
            error.to_string(),
            "This server runs Mattermost 9.4.9; Matty needs 9.5.0 or later."
        );
        caps.version = MINIMUM_VERSION;
        assert_eq!(caps.minimum_supported(), Ok(()));
    }

    #[test]
    fn a_config_without_a_version_is_an_error() {
        let mut config = load("config_client_team.json");
        config.remove("Version");
        assert!(ServerCapabilities::from_old_format(&config, &OldFormat::new()).is_err());
    }
}
