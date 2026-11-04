//! Native OS notifications, dock and taskbar badges, and sounds, one backend
//! per platform (PLAN §6.7).

pub mod backend;
pub mod mention;
pub mod policy;

pub use backend::Notifier;
pub use mention::MentionKeys;
pub use policy::{
    AppState, ChannelKind, ChannelPrefs, Event, Level, Notification, UserPrefs, decide,
};
