//! Notification sounds (#142): the Mattermost sound names the user picks
//! from, read from their `desktop_notification_sound` prop, and how each
//! one plays. macOS plays a system sound through NSSound; elsewhere there
//! is no system sound API to use yet, so [`play`] does nothing.

/// One of the web app's notification sounds, by its `notify_props` name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sound {
    /// Mattermost's default.
    #[default]
    Bing,
    /// Crackle.
    Crackle,
    /// Down.
    Down,
    /// Hello.
    Hello,
    /// Ripple.
    Ripple,
    /// Upstairs.
    Upstairs,
}

impl Sound {
    /// Every sound, in the order the web app lists them.
    pub const ALL: [Sound; 6] = [
        Sound::Bing,
        Sound::Crackle,
        Sound::Down,
        Sound::Hello,
        Sound::Ripple,
        Sound::Upstairs,
    ];

    /// Reads `desktop_notification_sound`: a name the web app writes, in
    /// any case. Missing or unknown is the default, Bing.
    pub fn from_prop(value: Option<&str>) -> Self {
        let value = value.unwrap_or("").trim();
        Self::ALL
            .into_iter()
            .find(|sound| sound.prop().eq_ignore_ascii_case(value))
            .unwrap_or_default()
    }

    /// The value for `desktop_notification_sound`, which is also its label.
    pub fn prop(self) -> &'static str {
        match self {
            Sound::Bing => "Bing",
            Sound::Crackle => "Crackle",
            Sound::Down => "Down",
            Sound::Hello => "Hello",
            Sound::Ripple => "Ripple",
            Sound::Upstairs => "Upstairs",
        }
    }

    /// The macOS system sound (`/System/Library/Sounds`) that stands in for
    /// it: the closest in character, since Matty does not ship the web
    /// app's audio files.
    pub fn system_name(self) -> &'static str {
        match self {
            Sound::Bing => "Glass",
            Sound::Crackle => "Frog",
            Sound::Down => "Bottle",
            Sound::Hello => "Hero",
            Sound::Ripple => "Submarine",
            Sound::Upstairs => "Purr",
        }
    }
}

/// Plays `sound` now: a notification's sound, or a preview from the
/// preferences. Only macOS has a backend.
pub fn play(sound: Sound) {
    #[cfg(target_os = "macos")]
    crate::macos::play(sound);
    #[cfg(not(target_os = "macos"))]
    tracing::debug!(?sound, "no system sound API on this platform");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_web_apps_names_with_bing_by_default() {
        assert_eq!(Sound::from_prop(None), Sound::Bing);
        assert_eq!(Sound::from_prop(Some("")), Sound::Bing);
        assert_eq!(Sound::from_prop(Some("Ripple")), Sound::Ripple);
        assert_eq!(Sound::from_prop(Some("upstairs")), Sound::Upstairs);
        assert_eq!(Sound::from_prop(Some(" Hello ")), Sound::Hello);
        assert_eq!(Sound::from_prop(Some("Klaxon")), Sound::Bing);
    }

    #[test]
    fn every_sound_round_trips_and_has_its_own_system_sound() {
        for sound in Sound::ALL {
            assert_eq!(Sound::from_prop(Some(sound.prop())), sound);
        }
        let mut names: Vec<_> = Sound::ALL.iter().map(|s| s.system_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Sound::ALL.len());
    }
}
