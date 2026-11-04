//! Native OS notifications, dock and taskbar badges, and sounds, one backend
//! per platform (PLAN §6.7).

pub mod backend;
#[cfg(target_os = "macos")]
mod macos;
pub mod mention;
pub mod policy;
pub mod sound;

pub use backend::{Badge, Clicked, Notifier};
pub use mention::MentionKeys;
pub use policy::{
    AppState, ChannelKind, ChannelPrefs, Event, Level, Notification, UserPrefs, decide,
};
pub use sound::{Sound, play as play_sound};
