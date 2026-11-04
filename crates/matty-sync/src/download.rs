//! Saving a post's file to the user's download folder (PLAN §4.6 Files).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use matty_api::Client;
use tokio::sync::mpsc::UnboundedSender;

use crate::change::DownloadState;
use crate::load::Loaded;

/// Fetches `file_id` into `folder` as it arrives, reporting progress, then
/// gives it a name of its own and reports where as [`Loaded::Download`].
pub(crate) async fn download(
    client: Client,
    file_id: String,
    folder: PathBuf,
    name: String,
    results: UnboundedSender<Loaded>,
) {
    let state = match save(&client, &file_id, &folder, &name, &results).await {
        Ok(path) => DownloadState::Done(path),
        Err(error) => DownloadState::Failed(error),
    };
    let _ = results.send(Loaded::Download { file_id, state });
}

/// Writes the body beside its final name and renames once it is whole, so
/// a half-written file never sits under it; the partial file goes on
/// failure.
async fn save(
    client: &Client,
    file_id: &str,
    folder: &Path,
    name: &str,
    results: &UnboundedSender<Loaded>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let partial = folder.join(format!(".{file_id}.partial"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => "it is already downloading".to_owned(),
            _ => error.to_string(),
        })?;
    let mut reported = None;
    let fetched = client
        .download_file(file_id, |bytes, received, total| {
            // Local writes are small and quick next to the network's pace.
            file.write_all(bytes)?;
            let step = progress_step(received, total);
            if reported != Some(step) {
                reported = Some(step);
                let _ = results.send(Loaded::Download {
                    file_id: file_id.to_owned(),
                    state: DownloadState::Progress { received, total },
                });
            }
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|_| file.sync_all().map_err(|error| error.to_string()));
    drop(file);
    let finished = fetched.and_then(|()| {
        let path = unique_path(folder, name);
        std::fs::rename(&partial, &path).map_err(|error| error.to_string())?;
        Ok(path)
    });
    if finished.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    finished
}

/// Which report a download at `received` of `total` bytes belongs to: one
/// per percent, or per megabyte when the size is unknown, so the screen is
/// not redrawn for every packet.
fn progress_step(received: u64, total: Option<u64>) -> u64 {
    match total {
        Some(total) if total > 0 => received.saturating_mul(100) / total,
        _ => received / (1024 * 1024),
    }
}

/// `folder/name`, or `name (1).ext` and up when that is taken. Only the
/// name's last component counts, so a server name never leaves `folder`.
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
