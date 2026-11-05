//! Open at login (#215, #216): the Preferences switch registers Matty
//! with the system so it starts when the person signs in. On macOS it is a
//! LaunchAgent, on Linux an XDG autostart entry, and on Windows the
//! current user's Run key. Each one names this copy of Matty, so it is
//! written again at every launch while the switch is on, which follows
//! the app when it moves.

use std::path::Path;

/// The file name, the Run value and the LaunchAgent label.
const LABEL: &str = "dev.arya.matty";

/// The LaunchAgent that starts `exe` at login.
pub fn launch_agent(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&exe.display().to_string())
    )
}

/// The XDG autostart entry that runs `exec` at login.
pub fn autostart_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Matty\nComment=Start Matty when you sign in\nExec={exec}\nIcon={LABEL}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// The Linux command that starts this copy: the Flatpak, the AppImage
/// file (not its mount point), or the binary, quoted as the desktop entry
/// spec asks.
pub fn linux_exec(flatpak_id: Option<&str>, appimage: Option<&str>, exe: &Path) -> String {
    if let Some(id) = flatpak_id.filter(|id| !id.is_empty()) {
        return format!("flatpak run {id}");
    }
    let path = appimage
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| exe.display().to_string());
    desktop_quote(&path)
}

/// Whether `exe` runs from somewhere it won't be at the next login: a
/// mounted disk image or macOS's translocated copy of a quarantined app.
pub fn is_transient(exe: &Path) -> bool {
    let path = exe.display().to_string();
    path.starts_with("/Volumes/") || path.contains("/AppTranslocation/")
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn desktop_quote(path: &str) -> String {
    if path
        .chars()
        .any(|c| c.is_whitespace() || "\"'\\$`".contains(c))
    {
        let escaped = path
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`");
        format!("\"{escaped}\"")
    } else {
        path.to_owned()
    }
}

/// Registers or removes the login item. It writes files or starts `reg`,
/// so call it off the main thread. Failures are logged.
pub fn apply(enabled: bool) {
    let result = platform::apply(enabled);
    match result {
        Ok(()) => tracing::info!(enabled, "open at login updated"),
        Err(error) => tracing::warn!(enabled, %error, "could not update open at login"),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::io;

    use super::{LABEL, is_transient, launch_agent};

    pub fn apply(enabled: bool) -> io::Result<()> {
        let home = dirs::home_dir().ok_or_else(|| io::Error::other("no home folder"))?;
        let dir = home.join("Library/LaunchAgents");
        let plist = dir.join(format!("{LABEL}.plist"));
        if !enabled {
            return remove(&plist);
        }
        let exe = std::env::current_exe()?;
        if is_transient(&exe) {
            return Err(io::Error::other(
                "Matty is running from a disk image; move it to Applications first",
            ));
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&plist, launch_agent(&exe))
    }

    fn remove(path: &std::path::Path) -> io::Result<()> {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io;

    use super::{LABEL, autostart_entry, linux_exec};

    pub fn apply(enabled: bool) -> io::Result<()> {
        let dir = dirs::config_dir()
            .ok_or_else(|| io::Error::other("no config folder"))?
            .join("autostart");
        let entry = dir.join(format!("{LABEL}.desktop"));
        if !enabled {
            return match std::fs::remove_file(&entry) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
        }
        let exe = std::env::current_exe()?;
        let exec = linux_exec(
            std::env::var("FLATPAK_ID").ok().as_deref(),
            std::env::var("APPIMAGE").ok().as_deref(),
            &exe,
        );
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&entry, autostart_entry(&exec))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::io;
    use std::process::Command;

    const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Matty";

    pub fn apply(enabled: bool) -> io::Result<()> {
        let mut reg = Command::new("reg");
        if enabled {
            let exe = std::env::current_exe()?;
            reg.args(["add", RUN, "/v", VALUE, "/t", "REG_SZ", "/f", "/d"])
                .arg(format!("\"{}\"", exe.display()));
        } else {
            reg.args(["delete", RUN, "/v", VALUE, "/f"]);
        }
        let status = reg.output()?.status;
        // Deleting a value that was never written fails too, which is fine.
        if status.success() || !enabled {
            Ok(())
        } else {
            Err(io::Error::other(format!("reg exited with {status}")))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    pub fn apply(_enabled: bool) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn the_launch_agent_runs_this_copy_at_load() {
        let plist = launch_agent(Path::new(
            "/Applications/Matty & Co.app/Contents/MacOS/matty",
        ));
        assert!(plist.contains("<string>dev.arya.matty</string>"));
        assert!(
            plist
                .contains("<string>/Applications/Matty &amp; Co.app/Contents/MacOS/matty</string>")
        );
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }

    #[test]
    fn the_autostart_entry_names_the_command() {
        let entry = autostart_entry("/usr/bin/matty");
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=/usr/bin/matty\n"));
        assert!(entry.contains("\nX-GNOME-Autostart-enabled=true\n"));
    }

    #[test]
    fn linux_starts_the_flatpak_then_the_appimage_then_the_binary() {
        let exe = Path::new("/tmp/.mount_Matty/usr/bin/matty");
        assert_eq!(
            linux_exec(Some("dev.arya.matty"), Some("/x.AppImage"), exe),
            "flatpak run dev.arya.matty"
        );
        assert_eq!(
            linux_exec(None, Some("/home/ada/Apps/Matty.AppImage"), exe),
            "/home/ada/Apps/Matty.AppImage"
        );
        assert_eq!(
            linux_exec(Some(""), None, Path::new("/usr/bin/matty")),
            "/usr/bin/matty"
        );
    }

    #[test]
    fn linux_quotes_a_path_with_spaces() {
        assert_eq!(
            linux_exec(
                None,
                Some("/home/ada/My Apps/Matty $1.AppImage"),
                Path::new("/x")
            ),
            r#""/home/ada/My Apps/Matty \$1.AppImage""#
        );
    }

    #[test]
    fn a_disk_image_or_translocated_copy_is_not_registered() {
        assert!(is_transient(Path::new(
            "/Volumes/Matty/Matty.app/Contents/MacOS/matty"
        )));
        assert!(is_transient(Path::new(
            "/private/var/folders/x/AppTranslocation/ABC/d/Matty.app/Contents/MacOS/matty"
        )));
        assert!(!is_transient(Path::new(
            "/Applications/Matty.app/Contents/MacOS/matty"
        )));
    }
}
