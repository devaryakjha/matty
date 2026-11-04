//! The updater against a local stand-in for GitHub Releases.

#![allow(clippy::unwrap_used)]

use aws_lc_rs::digest;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use matty_update::{Channel, UpdateError, Updater, Version};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ARCHIVE: &[u8] = b"PK not really a zip";

struct Feed {
    server: MockServer,
    key: Ed25519KeyPair,
}

impl Feed {
    async fn start() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Self {
            server: MockServer::start().await,
            key: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
        }
    }

    fn public_key(&self) -> String {
        BASE64.encode(self.key.public_key().as_ref())
    }

    fn manifest(&self, version: &str) -> String {
        let sha: String = digest::digest(&digest::SHA256, ARCHIVE)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        serde_json::json!({
            "version": version,
            "platforms": {
                "macos-universal": {
                    "url": format!("{}/download/Matty.zip", self.server.uri()),
                    "sha256": sha,
                    "size": ARCHIVE.len(),
                }
            }
        })
        .to_string()
    }

    /// Serves one release tagged `v{version}` whose manifest is `manifest`
    /// and whose signature covers `signed`.
    async fn release(&self, version: &str, prerelease: bool, manifest: &str, signed: &str) {
        let base = self.server.uri();
        let list = serde_json::json!([{
            "tag_name": format!("v{version}"),
            "draft": false,
            "prerelease": prerelease,
            "assets": [
                {"name": "matty-update.json", "browser_download_url": format!("{base}/download/manifest")},
                {"name": "matty-update.json.sig", "browser_download_url": format!("{base}/download/sig")},
            ]
        }]);
        Mock::given(method("GET"))
            .and(path("/repos/devaryakjha/matty/releases"))
            .respond_with(ResponseTemplate::new(200).set_body_json(list))
            .mount(&self.server)
            .await;
        Mock::given(path("/download/manifest"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest.to_owned()))
            .mount(&self.server)
            .await;
        let signature = BASE64.encode(self.key.sign(signed.as_bytes()).as_ref());
        Mock::given(path("/download/sig"))
            .respond_with(ResponseTemplate::new(200).set_body_string(signature + "\n"))
            .mount(&self.server)
            .await;
    }

    fn updater(&self, current: &str) -> Updater {
        Updater::new(
            &self.server.uri(),
            "devaryakjha/matty",
            &self.public_key(),
            current,
            "macos-universal",
        )
        .unwrap()
    }
}

#[tokio::test]
async fn an_alpha_finds_downloads_and_checks_the_next_beta() {
    let feed = Feed::start().await;
    let manifest = feed.manifest("0.1.0-alpha.2");
    feed.release("0.1.0-alpha.2", true, &manifest, &manifest)
        .await;
    Mock::given(path("/download/Matty.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(ARCHIVE))
        .mount(&feed.server)
        .await;
    let updater = feed.updater("0.1.0-alpha.1");

    let update = updater.check(Channel::Beta).await.unwrap().unwrap();
    assert_eq!(update.version, Version::parse("0.1.0-alpha.2").unwrap());
    // Stable never sees a pre-release.
    assert_eq!(updater.check(Channel::Stable).await.unwrap(), None);

    let dir = tempfile::tempdir().unwrap();
    let file = updater.download(&update, dir.path()).await.unwrap();
    assert_eq!(std::fs::read(file).unwrap(), ARCHIVE);
}

#[tokio::test]
async fn a_tampered_manifest_from_the_feed_is_rejected() {
    let feed = Feed::start().await;
    let signed = feed.manifest("0.1.0-alpha.2");
    let served = signed.replace("/download/Matty.zip", "/download/Evil.zip");
    feed.release("0.1.0-alpha.2", true, &served, &signed).await;
    let error = feed
        .updater("0.1.0-alpha.1")
        .check(Channel::Beta)
        .await
        .unwrap_err();
    assert!(matches!(error, UpdateError::BadSignature), "{error:?}");
}

#[tokio::test]
async fn a_download_that_does_not_match_is_deleted() {
    let feed = Feed::start().await;
    let manifest = feed.manifest("0.2.0");
    feed.release("0.2.0", false, &manifest, &manifest).await;
    Mock::given(path("/download/Matty.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK not really a zjp".as_slice()))
        .mount(&feed.server)
        .await;
    let updater = feed.updater("0.1.0");
    let update = updater.check(Channel::Stable).await.unwrap().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let error = updater.download(&update, dir.path()).await.unwrap_err();
    assert!(matches!(error, UpdateError::Checksum), "{error:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
