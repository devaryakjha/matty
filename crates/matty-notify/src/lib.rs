//! Native OS notifications, dock and taskbar badges, and sounds, one backend
//! per platform (PLAN §6.7).

pub mod backend;
#[cfg(target_os = "macos")]
mod macos;
pub mod mention;
pub mod policy;

pub use backend::{Clicked, Notifier};
pub use mention::MentionKeys;
pub use policy::{
    AppState, ChannelKind, ChannelPrefs, Event, Level, Notification, UserPrefs, decide,
};
