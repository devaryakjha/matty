//! The in-app updater (PLAN §10, #180): finds the newest release on a
//! channel, checks its signed manifest, and downloads the build it names.
//!
//! Every release the workflow signs carries two assets beside the DMG and
//! zip: `matty-update.json`, the [`Manifest`], and `matty-update.json.sig`,
//! the base64 ed25519 signature of that file's exact bytes. The key that
//! signs it lives only in the release workflow's secrets; the public half is
//! compiled in from `MATTY_UPDATE_PUBLIC_KEY` at build time, and a build
//! without it never checks ([`PUBLIC_KEY`]).
//!
//! Trust runs one way: the signature covers the manifest, and the manifest
//! names the archive's size and SHA-256. Neither the release list nor the
//! download host is trusted, so a feed that serves an old manifest or another
//! channel's gets nothing installed ([`offer`]).
//!
//! Plain Rust with no GPUI; the app drives [`Updater`] from its I/O thread.
//! The web build has no updater (the page is the latest build); it uses only
//! the version and channel.

#[cfg(not(target_family = "wasm"))]
pub mod install;

use std::collections::BTreeMap;
#[cfg(not(target_family = "wasm"))]
use std::io::Write as _;
#[cfg(not(target_family = "wasm"))]
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(not(target_family = "wasm"))]
use aws_lc_rs::digest;
#[cfg(not(target_family = "wasm"))]
use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
#[cfg(not(target_family = "wasm"))]
use base64::Engine as _;
#[cfg(not(target_family = "wasm"))]
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

pub use semver::Version;

/// This build's version: the release tag's (such as `0.1.0-beta.1`), which
/// the release workflow passes as `MATTY_VERSION`, or the workspace version
/// for a local build.
pub const CURRENT_VERSION: &str = match option_env!("MATTY_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// The base64 ed25519 public key (32 raw bytes) that update manifests must
/// be signed with, from `MATTY_UPDATE_PUBLIC_KEY` at build time. `None`
/// turns the updater off.
pub const PUBLIC_KEY: Option<&str> = option_env!("MATTY_UPDATE_PUBLIC_KEY");

/// The repository whose releases are the feed.
pub const REPOSITORY: &str = "devaryakjha/matty";
/// The GitHub REST API.
pub const GITHUB_API: &str = "https://api.github.com";
/// The manifest's asset name on a release.
pub const MANIFEST_ASSET: &str = "matty-update.json";
/// Its signature's asset name.
pub const SIGNATURE_ASSET: &str = "matty-update.json.sig";
/// How often a running app checks again.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[cfg(not(target_family = "wasm"))]
const MAX_MANIFEST: u64 = 64 * 1024;
#[cfg(not(target_family = "wasm"))]
const MAX_SIGNATURE: u64 = 1024;
#[cfg(not(target_family = "wasm"))]
const MAX_RELEASE_LIST: u64 = 4 * 1024 * 1024;
#[cfg(not(target_family = "wasm"))]
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;

/// What can stop a check, a download or an install.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// The compiled-in public key is not 32 base64 bytes.
    #[error("the update public key is not a base64 ed25519 key")]
    BadKey,
    /// The signature does not match the manifest and key.
    #[error("the update manifest's signature does not match")]
    BadSignature,
    /// A signed manifest, or the release list, makes no sense.
    #[error("the update manifest is not valid: {0}")]
    Malformed(String),
    /// A request failed.
    #[error("the update feed answered: {0}")]
    Http(String),
    /// A response or the archive ran past its limit.
    #[error("{0} is larger than expected")]
    TooLarge(&'static str),
    /// The archive's size or SHA-256 is not the manifest's.
    #[error("the download does not match the manifest's checksum")]
    Checksum,
    /// Writing the download or the bundle failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// This platform or build does not install updates.
    #[error("this build cannot install updates: {0}")]
    Unsupported(&'static str),
}

/// Which releases a person follows: the local `update_channel` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Releases without a pre-release suffix.
    Stable,
    /// Every release, pre-releases included, so beta never falls behind
    /// stable.
    Beta,
}

impl Channel {
    /// The channel a build follows until the person picks one: beta for a
    /// pre-release build (an alpha or beta), else stable.
    pub fn default_for(version: &str) -> Self {
        match Version::parse(version.trim_start_matches('v')) {
            Ok(version) if version.pre.is_empty() => Self::Stable,
            Ok(_) => Self::Beta,
            Err(_) => Self::Stable,
        }
    }

    /// Whether `version` is offered on this channel.
    pub fn accepts(self, version: &Version) -> bool {
        match self {
            Self::Stable => version.pre.is_empty(),
            Self::Beta => true,
        }
    }
}

/// The signed description of one release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The release's version without the `v`, such as `0.1.0-beta.2`.
    pub version: String,
    /// When it was published, RFC 3339.
    #[serde(default)]
    pub pub_date: Option<String>,
    /// The release page.
    #[serde(default)]
    pub notes_url: Option<String>,
    /// The archive for each platform, by [`platform`] key.
    pub platforms: BTreeMap<String, Archive>,
}

/// One platform's download.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archive {
    /// Where to download it.
    pub url: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    /// Its length in bytes.
    pub size: u64,
}

/// Checks `signature` (base64 ed25519 over the exact `bytes`) with
/// `public_key` and only then parses the manifest. Nothing in an unsigned or
/// altered manifest is ever read.
#[cfg(not(target_family = "wasm"))]
pub fn verify_manifest(
    bytes: &[u8],
    signature: &str,
    public_key: &str,
) -> Result<Manifest, UpdateError> {
    let key = BASE64
        .decode(public_key.trim())
        .ok()
        .filter(|key| key.len() == 32)
        .ok_or(UpdateError::BadKey)?;
    let signature = BASE64
        .decode(signature.trim())
        .map_err(|_| UpdateError::BadSignature)?;
    UnparsedPublicKey::new(&ED25519, &key)
        .verify(bytes, &signature)
        .map_err(|_| UpdateError::BadSignature)?;
    serde_json::from_slice(bytes).map_err(|error| UpdateError::Malformed(error.to_string()))
}

/// The archive key for this platform, or `None` where Matty does not update
/// itself yet (Linux and Windows packages update through their stores).
pub fn platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos-universal")
    } else {
        None
    }
}

/// An update this build should install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The version it installs.
    pub version: Version,
    /// What to download.
    pub archive: Archive,
    /// The release page.
    pub notes_url: Option<String>,
}

/// Decides whether a verified `manifest`, found on the release tagged
/// `tagged`, is an update for a build at `current` on `channel` and
/// `platform`. It must name the release it came from (so one release's
/// manifest cannot pass for another's), be on the channel, be newer, and
/// carry an archive for the platform.
pub fn offer(
    manifest: &Manifest,
    tagged: &Version,
    channel: Channel,
    current: &Version,
    platform: &str,
) -> Result<Option<Update>, UpdateError> {
    let version = Version::parse(&manifest.version)
        .map_err(|error| UpdateError::Malformed(format!("version: {error}")))?;
    if version != *tagged {
        return Err(UpdateError::Malformed(format!(
            "the manifest is for {version}, on the release for {tagged}"
        )));
    }
    if !channel.accepts(&version) || version <= *current {
        return Ok(None);
    }
    let Some(archive) = manifest.platforms.get(platform) else {
        return Ok(None);
    };
    if archive.sha256.len() != 64 || !archive.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UpdateError::Malformed("sha256".into()));
    }
    Ok(Some(Update {
        version,
        archive: archive.clone(),
        notes_url: manifest.notes_url.clone(),
    }))
}

/// One entry of GitHub's release list.
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    /// Such as `v0.1.0-beta.1`.
    pub tag_name: String,
    /// Not published yet.
    #[serde(default)]
    pub draft: bool,
    /// Marked pre-release on GitHub.
    #[serde(default)]
    pub prerelease: bool,
    /// Its files.
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseAsset {
    /// The file name.
    pub name: String,
    /// Where to download it.
    pub browser_download_url: String,
}

/// The release to look at for `channel`: the highest version newer than
/// `current` that carries a manifest and its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The release's version, from its tag.
    pub version: Version,
    /// Its `matty-update.json`.
    pub manifest_url: String,
    /// Its `matty-update.json.sig`.
    pub signature_url: String,
}

/// Picks the [`Candidate`] from a release list. Drafts and tags that are
/// not `v` + semver are skipped; stable skips anything marked pre-release
/// either by its version or by GitHub.
pub fn pick(releases: &[Release], channel: Channel, current: &Version) -> Option<Candidate> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| channel == Channel::Beta || !release.prerelease)
        .filter_map(|release| {
            let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
            if !channel.accepts(&version) || version <= *current {
                return None;
            }
            let url = |name: &str| {
                release
                    .assets
                    .iter()
                    .find(|asset| asset.name == name)
                    .map(|asset| asset.browser_download_url.clone())
            };
            Some(Candidate {
                version,
                manifest_url: url(MANIFEST_ASSET)?,
                signature_url: url(SIGNATURE_ASSET)?,
            })
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Lowercase hex of `bytes`.
#[cfg(not(target_family = "wasm"))]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Checks the feed and downloads updates for one build.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Debug)]
pub struct Updater {
    http: reqwest::Client,
    api: String,
    repository: String,
    public_key: String,
    current: Version,
    platform: String,
}

#[cfg(not(target_family = "wasm"))]
impl Updater {
    /// An updater for this build, or `None` when it cannot update itself:
    /// no compiled-in key, a platform without archives, or a version that is
    /// not semver.
    pub fn for_this_build() -> Option<Self> {
        Self::new(
            GITHUB_API,
            REPOSITORY,
            PUBLIC_KEY.filter(|key| !key.trim().is_empty())?,
            CURRENT_VERSION,
            platform()?,
        )
        .inspect_err(|error| tracing::warn!(%error, "the updater could not start"))
        .ok()
    }

    /// An updater against any API base, for tests.
    pub fn new(
        api: &str,
        repository: &str,
        public_key: &str,
        current: &str,
        platform: &str,
    ) -> Result<Self, UpdateError> {
        let current = Version::parse(current)
            .map_err(|error| UpdateError::Malformed(format!("this build's version: {error}")))?;
        let tls =
            matty_api::https_config().map_err(|error| UpdateError::Http(error.to_string()))?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("Matty/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .tls_backend_preconfigured(tls)
            .build()
            .map_err(|error| UpdateError::Http(error.to_string()))?;
        Ok(Self {
            http,
            api: api.trim_end_matches('/').to_owned(),
            repository: repository.to_owned(),
            public_key: public_key.to_owned(),
            current,
            platform: platform.to_owned(),
        })
    }

    /// This build's version.
    pub fn current(&self) -> &Version {
        &self.current
    }

    /// Looks for an update on `channel`. `Ok(None)` means this build is the
    /// newest there.
    pub async fn check(&self, channel: Channel) -> Result<Option<Update>, UpdateError> {
        let url = format!(
            "{}/repos/{}/releases?per_page=30",
            self.api, self.repository
        );
        let list = self
            .get(&url, MAX_RELEASE_LIST, "the release list", |request| {
                request
                    .header("accept", "application/vnd.github+json")
                    .timeout(Duration::from_secs(30))
            })
            .await?;
        let releases: Vec<Release> = serde_json::from_slice(&list)
            .map_err(|error| UpdateError::Malformed(format!("release list: {error}")))?;
        let Some(candidate) = pick(&releases, channel, &self.current) else {
            return Ok(None);
        };
        let quick = |request: reqwest::RequestBuilder| request.timeout(Duration::from_secs(30));
        let manifest = self
            .get(&candidate.manifest_url, MAX_MANIFEST, "the manifest", quick)
            .await?;
        let signature = self
            .get(
                &candidate.signature_url,
                MAX_SIGNATURE,
                "the signature",
                quick,
            )
            .await?;
        let signature = String::from_utf8_lossy(&signature);
        let manifest = verify_manifest(&manifest, &signature, &self.public_key)?;
        offer(
            &manifest,
            &candidate.version,
            channel,
            &self.current,
            &self.platform,
        )
    }

    /// Downloads `update`'s archive into `dir` and checks its size and
    /// SHA-256 against the manifest. A file that does not match is deleted.
    pub async fn download(&self, update: &Update, dir: &Path) -> Result<PathBuf, UpdateError> {
        let expected = update.archive.size;
        if expected > MAX_ARCHIVE {
            return Err(UpdateError::TooLarge("the archive"));
        }
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("Matty-{}.zip", update.version));
        let partial = dir.join(format!("Matty-{}.zip.part", update.version));
        let result = async {
            let mut response = self
                .http
                .get(&update.archive.url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|error| UpdateError::Http(error.to_string()))?;
            let mut file = std::fs::File::create(&partial)?;
            let mut hash = digest::Context::new(&digest::SHA256);
            let mut length = 0u64;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| UpdateError::Http(error.to_string()))?
            {
                length += chunk.len() as u64;
                if length > expected {
                    return Err(UpdateError::TooLarge("the archive"));
                }
                hash.update(&chunk);
                file.write_all(&chunk)?;
            }
            file.sync_all()?;
            if length != expected
                || !hex(hash.finish().as_ref()).eq_ignore_ascii_case(&update.archive.sha256)
            {
                return Err(UpdateError::Checksum);
            }
            std::fs::rename(&partial, &path)?;
            Ok(path.clone())
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        result
    }

    async fn get(
        &self,
        url: &str,
        limit: u64,
        what: &'static str,
        build: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<Vec<u8>, UpdateError> {
        let mut response = build(self.http.get(url))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| UpdateError::Http(error.to_string()))?;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| UpdateError::Http(error.to_string()))?
        {
            if (body.len() + chunk.len()) as u64 > limit {
                return Err(UpdateError::TooLarge(what));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};

    fn key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn public(key: &Ed25519KeyPair) -> String {
        BASE64.encode(key.public_key().as_ref())
    }

    fn sign(key: &Ed25519KeyPair, bytes: &[u8]) -> String {
        BASE64.encode(key.sign(bytes).as_ref())
    }

    const MANIFEST: &str = r#"{"version":"0.2.0-beta.1","platforms":{"macos-universal":{"url":"https://example.com/Matty.zip","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size":3}}}"#;

    fn v(version: &str) -> Version {
        Version::parse(version).unwrap()
    }

    #[test]
    fn a_signed_manifest_verifies() {
        let key = key();
        let manifest = verify_manifest(
            MANIFEST.as_bytes(),
            &sign(&key, MANIFEST.as_bytes()),
            &public(&key),
        )
        .unwrap();
        assert_eq!(manifest.version, "0.2.0-beta.1");
        assert_eq!(manifest.platforms["macos-universal"].size, 3);
    }

    #[test]
    fn a_tampered_manifest_is_rejected() {
        let key = key();
        let signature = sign(&key, MANIFEST.as_bytes());
        let tampered = MANIFEST.replace("example.com", "evil.example");
        assert!(matches!(
            verify_manifest(tampered.as_bytes(), &signature, &public(&key)),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn another_keys_signature_is_rejected() {
        let signature = sign(&key(), MANIFEST.as_bytes());
        assert!(matches!(
            verify_manifest(MANIFEST.as_bytes(), &signature, &public(&key())),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn a_garbled_signature_or_key_is_rejected() {
        let key = key();
        assert!(matches!(
            verify_manifest(MANIFEST.as_bytes(), "not base64!", &public(&key)),
            Err(UpdateError::BadSignature)
        ));
        assert!(matches!(
            verify_manifest(MANIFEST.as_bytes(), "", &public(&key)),
            Err(UpdateError::BadSignature)
        ));
        assert!(matches!(
            verify_manifest(
                MANIFEST.as_bytes(),
                &sign(&key, MANIFEST.as_bytes()),
                "AAAA"
            ),
            Err(UpdateError::BadKey)
        ));
    }

    #[test]
    fn a_signature_made_by_openssl_verifies() {
        // `openssl genpkey -algorithm ed25519` and `openssl pkeyutl -sign
        // -rawin`, as the release workflow signs, over "matty\n".
        let public = "BNg/YSknS8OXSqtrdsS+MiqPkMUVDK/KIx/+ZcSvDNI=";
        let signature = "UmQ+uNpo1VFcVMrCsczSi3QJvQcAa8dU0zl28x26tn1xu/IqwWinBPIHRlL5cmY06UnFBHOWjjtEHbYTakzOBQ==";
        let key = BASE64.decode(public).unwrap();
        let signature = BASE64.decode(signature).unwrap();
        assert!(
            UnparsedPublicKey::new(&ED25519, &key)
                .verify(b"matty\n", &signature)
                .is_ok()
        );
    }

    #[test]
    fn pre_release_builds_default_to_beta() {
        assert_eq!(Channel::default_for("0.1.0-alpha.3"), Channel::Beta);
        assert_eq!(Channel::default_for("v0.1.0-beta.1"), Channel::Beta);
        assert_eq!(Channel::default_for("0.1.0"), Channel::Stable);
        assert_eq!(Channel::default_for("not a version"), Channel::Stable);
    }

    #[test]
    fn versions_order_pre_releases_before_their_release() {
        assert!(v("0.1.0-alpha.2") > v("0.1.0-alpha.1"));
        assert!(v("0.1.0-alpha.10") > v("0.1.0-alpha.9"));
        assert!(v("0.1.0-beta.1") > v("0.1.0-alpha.9"));
        assert!(v("0.1.0") > v("0.1.0-beta.3"));
        assert!(v("0.1.1-alpha.1") > v("0.1.0"));
    }

    fn release(tag: &str, prerelease: bool, signed: bool) -> Release {
        let mut assets = vec![ReleaseAsset {
            name: "Matty.dmg".into(),
            browser_download_url: format!("https://dl/{tag}/Matty.dmg"),
        }];
        if signed {
            for name in [MANIFEST_ASSET, SIGNATURE_ASSET] {
                assets.push(ReleaseAsset {
                    name: name.into(),
                    browser_download_url: format!("https://dl/{tag}/{name}"),
                });
            }
        }
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease,
            assets,
        }
    }

    #[test]
    fn each_channel_picks_its_newest_signed_release() {
        let mut draft = release("v0.4.0", false, true);
        draft.draft = true;
        let releases = vec![
            release("v0.2.0-beta.1", true, true),
            draft,
            release("v0.3.0-beta.1", true, false),
            release("v0.1.1", false, true),
            release("nightly", true, true),
            release("v0.1.0", false, true),
        ];
        let current = v("0.1.0-alpha.1");
        let beta = pick(&releases, Channel::Beta, &current).unwrap();
        assert_eq!(beta.version, v("0.2.0-beta.1"));
        assert_eq!(
            beta.manifest_url,
            "https://dl/v0.2.0-beta.1/matty-update.json"
        );
        let stable = pick(&releases, Channel::Stable, &current).unwrap();
        assert_eq!(stable.version, v("0.1.1"));
        assert_eq!(pick(&releases, Channel::Beta, &v("0.2.0-beta.1")), None);
        // GitHub's pre-release flag alone keeps a release off stable.
        let flagged = vec![release("v9.0.0", true, true)];
        assert_eq!(pick(&flagged, Channel::Stable, &current), None);
    }

    fn manifest(version: &str) -> Manifest {
        serde_json::from_str(&MANIFEST.replace("0.2.0-beta.1", version)).unwrap()
    }

    #[test]
    fn offer_needs_a_newer_version_on_the_channel_for_this_platform() {
        let current = v("0.1.0");
        let found = offer(
            &manifest("0.1.1"),
            &v("0.1.1"),
            Channel::Stable,
            &current,
            "macos-universal",
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.version, v("0.1.1"));
        // Not newer.
        assert_eq!(
            offer(
                &manifest("0.1.0"),
                &v("0.1.0"),
                Channel::Beta,
                &current,
                "macos-universal"
            )
            .unwrap(),
            None
        );
        // A beta manifest replayed on the stable feed.
        assert_eq!(
            offer(
                &manifest("0.2.0-beta.1"),
                &v("0.2.0-beta.1"),
                Channel::Stable,
                &current,
                "macos-universal"
            )
            .unwrap(),
            None
        );
        // No archive for this platform.
        assert_eq!(
            offer(
                &manifest("0.1.1"),
                &v("0.1.1"),
                Channel::Stable,
                &current,
                "linux-x86_64"
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn a_manifest_on_another_release_is_rejected() {
        // An old signed manifest attached to a newer tag.
        assert!(matches!(
            offer(
                &manifest("0.1.1"),
                &v("0.9.0"),
                Channel::Stable,
                &v("0.1.0"),
                "macos-universal"
            ),
            Err(UpdateError::Malformed(_))
        ));
    }
}
