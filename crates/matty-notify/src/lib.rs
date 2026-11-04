//! Native OS notifications, dock and taskbar badges, and sounds, one backend
//! per platform (PLAN §6.7).

pub mod backend;
#[cfg(target_os = "macos")]
mod macos;
pub mod mention;
pub mod policy;

pub use backend::{Badge, Clicked, Notifier};
pub use mention::{KeywordList, MentionKeys};
pub use policy::{
    AppState, ChannelKind, ChannelPrefs, Event, Level, Notification, UserPrefs, decide, dnd_active,
};
