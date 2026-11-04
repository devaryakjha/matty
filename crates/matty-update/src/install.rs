//! Putting a downloaded update in place and starting it.
//!
//! On macOS the archive is a zip of `Matty.app`. [`stage`] unpacks it with
//! `ditto` next to the download and checks its code signature;
//! [`install_and_relaunch`] swaps it for the running bundle and starts it
//! once this process has quit. Other platforms update through their package
//! managers, so both refuse there.

use std::path::{Path, PathBuf};

use crate::UpdateError;

/// The `.app` bundle this process runs from, or `None` for a binary outside
/// one (a `cargo run` build), which has nothing to replace.
pub fn current_bundle() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    bundle_of(&exe)
}

/// The bundle that holds `exe` at `Name.app/Contents/MacOS/exe`.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Unpacks `archive` into a fresh folder beside it and returns the
/// `Matty.app` inside, once `codesign` accepts it.
#[cfg(target_os = "macos")]
pub fn stage(archive: &Path) -> Result<PathBuf, UpdateError> {
    use std::process::Command;

    let dir = archive.with_extension("staged");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(&dir))?;
    let app = dir.join("Matty.app");
    if !app.join("Contents/MacOS/matty").is_file() {
        return Err(UpdateError::Malformed(
            "the archive holds no Matty.app".into(),
        ));
    }
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&app))?;
    Ok(app)
}

/// Not on this platform.
#[cfg(not(target_os = "macos"))]
pub fn stage(_archive: &Path) -> Result<PathBuf, UpdateError> {
    Err(UpdateError::Unsupported("updates install on macOS only"))
}

/// Moves `staged` over `bundle`, keeping the old bundle until the new one is
/// in place, then starts a shell that opens the new bundle once this
/// process exits. The caller quits the app next.
#[cfg(target_os = "macos")]
pub fn install_and_relaunch(staged: &Path, bundle: &Path) -> Result<(), UpdateError> {
    use std::process::Command;

    let parent = bundle
        .parent()
        .ok_or(UpdateError::Unsupported("the app has no parent folder"))?;
    let old = parent.join(format!(".Matty-old-{}.app", std::process::id()));
    // The running process keeps its open files when its bundle moves.
    std::fs::rename(bundle, &old)?;
    let moved = std::fs::rename(staged, bundle).or_else(|_| {
        // Another volume: copy instead.
        run(Command::new("/usr/bin/ditto").arg(staged).arg(bundle))
            .map_err(|error| std::io::Error::other(error.to_string()))
    });
    if let Err(error) = moved {
        let _ = std::fs::remove_dir_all(bundle);
        let _ = std::fs::rename(&old, bundle);
        return Err(error.into());
    }
    let _ = std::fs::remove_dir_all(&old);
    Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#)
        .arg("matty-relaunch")
        .arg(std::process::id().to_string())
        .arg(bundle)
        .spawn()?;
    Ok(())
}

/// Not on this platform.
#[cfg(not(target_os = "macos"))]
pub fn install_and_relaunch(_staged: &Path, _bundle: &Path) -> Result<(), UpdateError> {
    Err(UpdateError::Unsupported("updates install on macOS only"))
}

#[cfg(target_os = "macos")]
fn run(command: &mut std::process::Command) -> Result<(), UpdateError> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(UpdateError::Malformed(format!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_three_folders_up_from_the_binary() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Matty.app/Contents/MacOS/matty")),
            Some(PathBuf::from("/Applications/Matty.app"))
        );
        assert_eq!(
            bundle_of(Path::new("/home/me/matty/target/debug/matty")),
            None
        );
        assert_eq!(bundle_of(Path::new("/matty")), None);
    }
}
