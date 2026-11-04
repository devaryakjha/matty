//! Saving a post's file to the user's download folder (PLAN §4.6 Files).

use std::path::{Path, PathBuf};

use matty_api::{Client, Fetched};
use tokio::sync::mpsc::UnboundedSender;

use crate::change::DownloadState;
use crate::load::Loaded;

/// Fetches `file_id` and writes it into `folder` under a name of its own,
/// then reports where as [`Loaded::Download`].
pub(crate) async fn download(
    client: Client,
    file_id: String,
    folder: PathBuf,
    name: String,
    results: UnboundedSender<Loaded>,
) {
    let state = match client.file(&file_id, None).await {
        #[cfg(target_family = "wasm")]
        Ok(Fetched::Fresh { bytes, .. }) => {
            let _ = folder;
            match crate::web::save_download(&name, &bytes) {
                Ok(()) => DownloadState::Done(PathBuf::from(name)),
                Err(error) => DownloadState::Failed(error),
            }
        }
        #[cfg(not(target_family = "wasm"))]
        Ok(Fetched::Fresh { bytes, .. }) => {
            match tokio::task::spawn_blocking(move || save(&folder, &name, &bytes)).await {
                Ok(Ok(path)) => DownloadState::Done(path),
                Ok(Err(error)) => DownloadState::Failed(error.to_string()),
                Err(error) => DownloadState::Failed(error.to_string()),
            }
        }
        Ok(Fetched::NotModified) => DownloadState::Failed("the server sent nothing".into()),
        Err(error) => DownloadState::Failed(error.to_string()),
    };
    let _ = results.send(Loaded::Download { file_id, state });
}

/// Writes `bytes` beside their final name and renames, so a half-written
/// file never sits under it.
#[cfg(not(target_family = "wasm"))]
fn save(folder: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let path = unique_path(folder, name);
    let partial = path.with_file_name(format!(
        ".{}.partial",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// `folder/name`, or `name (1).ext` and up when that is taken. Only the
/// name's last component counts, so a server name never leaves `folder`.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn unique_path(folder: &Path, name: &str) -> PathBuf {
    let name = Path::new(name)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .unwrap_or_else(|| "file".to_owned());
    let path = folder.join(&name);
    if !path.exists() {
        return path;
    }
    let file = Path::new(&name);
    let stem = file
        .file_stem()
        .map_or_else(|| name.clone(), |stem| stem.to_string_lossy().into_owned());
    let extension = file
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| folder.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_taken_name_gets_a_number() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_path(dir.path(), "menu.png"),
            dir.path().join("menu.png")
        );
        std::fs::write(dir.path().join("menu.png"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "menu.png"),
            dir.path().join("menu (1).png")
        );
        std::fs::write(dir.path().join("menu (1).png"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "menu.png"),
            dir.path().join("menu (2).png")
        );
        std::fs::write(dir.path().join("README"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "README"),
            dir.path().join("README (1)")
        );
    }

    #[test]
    fn a_name_never_leaves_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_path(dir.path(), "../../.bashrc"),
            dir.path().join("file")
        );
        assert_eq!(
            unique_path(dir.path(), "/etc/passwd"),
            dir.path().join("passwd")
        );
        assert_eq!(unique_path(dir.path(), ".."), dir.path().join("file"));
    }
}
