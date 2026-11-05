//! Who opens `mattermost://` links on this computer (#489). The server
//! hands the SSO token to whichever app holds the scheme and never checks
//! the client token, so when the official app holds it, Matty's browser
//! sign-in never comes back. The sign-in card asks here, says so, and
//! offers to make Matty the handler. Matty never takes the scheme back
//! without the person asking.

/// The scheme the server's SSO hand-off uses.
pub const SCHEME: &str = "mattermost";

/// Who holds the scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// Matty itself, or nothing to worry about.
    Matty,
    /// No app holds it, or the system would not say.
    Nobody,
    /// Another app, by the name to show.
    Other(String),
}

/// Decides who `handler` is: an app bundle path on macOS, a desktop file
/// id on Linux, or an open command on Windows. `ours` are the strings that
/// name Matty there, compared without case.
pub fn decide(handler: Option<&str>, ours: &[&str]) -> Owner {
    let Some(handler) = handler.map(str::trim).filter(|handler| !handler.is_empty()) else {
        return Owner::Nobody;
    };
    let lower = handler.to_lowercase();
    if ours
        .iter()
        .filter(|ours| !ours.is_empty())
        .any(|ours| lower.contains(&ours.to_lowercase()))
    {
        return Owner::Matty;
    }
    Owner::Other(app_name(handler))
}

/// The name to show for `handler`: its file name without the folder,
/// the extension, or a command's arguments.
fn app_name(handler: &str) -> String {
    let path = match handler.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(quoted),
        None => handler.split_whitespace().next().unwrap_or(handler),
    };
    let file = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path);
    let name = [".app", ".desktop", ".exe"]
        .iter()
        .find_map(|extension| {
            file.len()
                .checked_sub(extension.len())
                .filter(|&at| file[at..].eq_ignore_ascii_case(extension))
                .map(|at| &file[..at])
        })
        .unwrap_or(file);
    // Reverse-domain desktop ids, such as com.mattermost.Desktop, read by
    // their last part.
    let name = if handler.ends_with(".desktop") {
        name.rsplit('.').next().unwrap_or(name)
    } else {
        name
    };
    if name.is_empty() {
        handler.to_owned()
    } else {
        name.to_owned()
    }
}

/// Asks the system who holds the scheme. It can start a process, so call
/// it off the main thread.
pub fn owner() -> Owner {
    platform::owner()
}

/// Makes Matty the handler: macOS asks with its own confirmation, Windows
/// opens Default apps for the person to choose, and Linux sets it for this
/// user. Errors are logged; the card checks again afterwards.
pub fn claim() {
    platform::claim();
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSError, NSString, NSURL};

    use super::{Owner, SCHEME, decide};

    fn probe() -> Option<objc2::rc::Retained<NSURL>> {
        NSURL::URLWithString(&NSString::from_str(&format!("{SCHEME}://")))
    }

    pub fn owner() -> Owner {
        let Some(probe) = probe() else {
            return Owner::Nobody;
        };
        let handler = NSWorkspace::sharedWorkspace()
            .URLForApplicationToOpenURL(&probe)
            .and_then(|app| app.path())
            .map(|path| path.to_string());
        let ours = NSBundle::mainBundle()
            .bundleURL()
            .path()
            .map(|path| path.to_string())
            .unwrap_or_default();
        decide(handler.as_deref(), &[&ours])
    }

    pub fn claim() {
        let app = NSBundle::mainBundle().bundleURL();
        let handler = block2::RcBlock::new(|error: *mut NSError| {
            if error.is_null() {
                tracing::info!("Matty now opens {SCHEME}:// links");
            } else {
                tracing::warn!("macOS did not make Matty the {SCHEME}:// handler");
            }
        });
        NSWorkspace::sharedWorkspace()
            .setDefaultApplicationAtURL_toOpenURLsWithScheme_completionHandler(
                &app,
                &NSString::from_str(SCHEME),
                Some(&handler),
            );
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::process::Command;

    use super::{Owner, SCHEME, decide};

    /// The desktop file `packaging/linux` installs.
    const DESKTOP_ID: &str = "dev.arya.matty.desktop";

    fn mime() -> String {
        format!("x-scheme-handler/{SCHEME}")
    }

    pub fn owner() -> Owner {
        let handler = Command::new("xdg-mime")
            .args(["query", "default", &mime()])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
        decide(handler.as_deref(), &[DESKTOP_ID])
    }

    pub fn claim() {
        match Command::new("xdg-mime")
            .args(["default", DESKTOP_ID, &mime()])
            .status()
        {
            Ok(status) if status.success() => tracing::info!("Matty now opens {SCHEME}:// links"),
            other => tracing::warn!(
                ?other,
                "xdg-mime did not make Matty the {SCHEME}:// handler"
            ),
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::process::Command;

    use super::{Owner, SCHEME, decide};

    /// The open command registered for the scheme; HKCR merges the user's
    /// registration over the machine's.
    pub fn owner() -> Owner {
        let key = format!(r"HKCR\{SCHEME}\shell\open\command");
        let handler = Command::new("reg")
            .args(["query", &key, "/ve"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .find_map(|line| {
                        line.split_once("REG_SZ")
                            .map(|(_, value)| value.trim().to_owned())
                    })
            });
        let ours = std::env::current_exe()
            .map(|exe| exe.display().to_string())
            .unwrap_or_default();
        decide(handler.as_deref(), &[&ours])
    }

    /// Windows lets only the person change a protocol's default, so this
    /// opens Default apps.
    pub fn claim() {
        if let Err(error) = Command::new("explorer")
            .arg("ms-settings:defaultapps")
            .spawn()
        {
            tracing::warn!(%error, "could not open Default apps");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use super::Owner;

    pub fn owner() -> Owner {
        Owner::Nobody
    }

    pub fn claim() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_handler_is_nobody() {
        assert_eq!(decide(None, &["dev.arya.matty.desktop"]), Owner::Nobody);
        assert_eq!(
            decide(Some("  \n"), &["dev.arya.matty.desktop"]),
            Owner::Nobody
        );
    }

    #[test]
    fn matty_holding_the_scheme_needs_no_notice() {
        assert_eq!(
            decide(
                Some("dev.arya.matty.desktop\n"),
                &["dev.arya.matty.desktop"]
            ),
            Owner::Matty
        );
        assert_eq!(
            decide(
                Some("/Applications/Matty.app"),
                &["/Applications/Matty.app"]
            ),
            Owner::Matty
        );
        assert_eq!(
            decide(
                Some(r#""C:\Users\ada\AppData\Local\Matty\matty.exe" "%1""#),
                &[r"c:\users\ada\appdata\local\matty\matty.exe"],
            ),
            Owner::Matty
        );
    }

    #[test]
    fn another_app_is_named_from_its_path() {
        assert_eq!(
            decide(
                Some("/Applications/Mattermost.app"),
                &["/Applications/Matty.app"]
            ),
            Owner::Other("Mattermost".into())
        );
        assert_eq!(
            decide(Some("Mattermost.desktop\n"), &["dev.arya.matty.desktop"]),
            Owner::Other("Mattermost".into())
        );
        assert_eq!(
            decide(
                Some("com.mattermost.Desktop.desktop"),
                &["dev.arya.matty.desktop"]
            ),
            Owner::Other("Desktop".into())
        );
        assert_eq!(
            decide(
                Some(r#""C:\Program Files\Mattermost\Mattermost.exe" -- "%1""#),
                &[r"C:\Matty\matty.exe"],
            ),
            Owner::Other("Mattermost".into())
        );
    }

    #[test]
    fn an_unknown_matty_path_does_not_match_everything() {
        // A failed lookup of Matty's own path must not make every handler
        // look like Matty.
        assert_eq!(
            decide(Some("/Applications/Mattermost.app"), &[""]),
            Owner::Other("Mattermost".into())
        );
    }
}
