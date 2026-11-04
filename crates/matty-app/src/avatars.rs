//! Profile pictures for every avatar Matty draws (#153).
//!
//! Views ask [`image`] for a user's picture while they render. The first ask
//! for a user sends sync one `LoadAvatar`; sync answers from the media cache
//! on disk or fetches `users/{id}/image` and keeps it there, so pictures
//! show offline. Until the answer lands, and when there is none, the view
//! keeps the gpuicn avatar's initials, which take the same slot, so nothing
//! moves when the picture arrives.
//!
//! Pictures are keyed by user id and `last_picture_update`: when a user
//! changes theirs (the `user_updated` event carries the new time), the next
//! ask fetches the new one while the old one stays up. The bytes become an
//! [`Image`] here without decoding; GPUI decodes it on its background
//! executor the first time it is drawn, so no picture is fetched or decoded
//! on the main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Global, Image};

use crate::ui::avatar::Avatar;

/// Sends sync the request for one user's picture: the version the app
/// knows of, or `None` to let sync use the one it has stored.
pub type Ask = Rc<dyn Fn(&str, Option<i64>)>;

/// The pictures known so far, shared between [`crate::live::Live`], which
/// learns versions and receives answers, and the views, which read it
/// through the app global while rendering. Everything runs on the main
/// thread.
#[derive(Clone, Default)]
pub struct Avatars(Rc<RefCell<Store>>);

impl Global for Avatars {}

#[derive(Default)]
struct Store {
    entries: HashMap<String, Entry>,
    ask: Option<Ask>,
}

/// One user's picture.
#[derive(Default)]
struct Entry {
    /// The newest `last_picture_update` the app has heard of.
    known: Option<i64>,
    /// What was last asked of sync, and whether its answer came back
    /// empty. `Some(None)` asked for whatever version sync had stored.
    asked: Option<Option<i64>>,
    failed: bool,
    /// The picture on screen and its version. An older one stays up
    /// while a newer one loads.
    image: Option<(i64, Arc<Image>)>,
}

impl Entry {
    /// Whether the picture for the newest known version is missing and
    /// not yet asked for.
    fn wants(&self) -> bool {
        let current = match (self.known, &self.image) {
            (Some(known), Some((version, _))) => *version == known,
            (None, Some(_)) => true,
            (_, None) => false,
        };
        !current && self.asked != Some(self.known)
    }
}

impl Avatars {
    /// A store that asks sync through `ask`.
    pub fn new(ask: Ask) -> Self {
        Self(Rc::new(RefCell::new(Store {
            entries: HashMap::new(),
            ask: Some(ask),
        })))
    }

    /// The user's picture, asking sync for it the first time and again
    /// when their picture changed. `None` means initials for now.
    pub fn image(&self, user_id: &str) -> Option<Arc<Image>> {
        if user_id.is_empty() {
            return None;
        }
        let mut store = self.0.borrow_mut();
        let ask = store.ask.clone();
        let entry = store.entries.entry(user_id.to_owned()).or_default();
        if entry.wants()
            && let Some(ask) = ask
        {
            entry.asked = Some(entry.known);
            entry.failed = false;
            ask(user_id, entry.known);
        }
        entry.image.as_ref().map(|(_, image)| image.clone())
    }

    /// Notes a user's `last_picture_update`, from any user record sync
    /// sends. A change for someone already on screen asks for the new
    /// picture at once.
    pub fn learn(&self, user_id: &str, last_picture_update: i64) {
        let mut store = self.0.borrow_mut();
        let ask = store.ask.clone();
        let Some(entry) = store.entries.get_mut(user_id) else {
            store.entries.insert(
                user_id.to_owned(),
                Entry {
                    known: Some(last_picture_update),
                    ..Entry::default()
                },
            );
            return;
        };
        entry.known = Some(last_picture_update);
        if entry.asked.is_some()
            && entry.wants()
            && let Some(ask) = ask
        {
            entry.asked = Some(entry.known);
            entry.failed = false;
            ask(user_id, entry.known);
        }
    }

    /// Sync's answer for `user_id` at `version`: the picture, or `None`
    /// when there is none to show. Returns whether what views draw changed.
    pub fn landed(&self, user_id: &str, version: i64, image: Option<Arc<Image>>) -> bool {
        let mut store = self.0.borrow_mut();
        let entry = store.entries.entry(user_id.to_owned()).or_default();
        match image {
            // An answer older than the picture up already is stale.
            Some(image)
                if entry
                    .image
                    .as_ref()
                    .is_none_or(|(shown, _)| *shown <= version) =>
            {
                entry.image = Some((version, image));
                entry.failed = false;
                if entry.known.is_none() {
                    entry.known = Some(version);
                }
                true
            }
            Some(_) => false,
            None => {
                entry.failed = true;
                false
            }
        }
    }

    /// Asks again for the pictures whose last answer came back empty, as
    /// when the connection returns.
    pub fn retry_failed(&self) {
        let mut store = self.0.borrow_mut();
        let ask = store.ask.clone();
        let Some(ask) = ask else {
            return;
        };
        for (user_id, entry) in &mut store.entries {
            if entry.failed {
                entry.failed = false;
                entry.asked = Some(entry.known);
                ask(user_id, entry.known);
            }
        }
    }

    /// Puts a picture up directly, for the preview fixtures.
    pub fn insert(&self, user_id: &str, image: Arc<Image>) {
        self.landed(user_id, 0, Some(image));
    }
}

/// The user's picture from the app's [`Avatars`], if one is up.
pub fn image(user_id: &str, cx: &App) -> Option<Arc<Image>> {
    cx.try_global::<Avatars>()?.image(user_id)
}

/// `avatar` with the user's picture once it has one; until then it keeps
/// its initials.
pub fn with_picture(avatar: Avatar, user_id: &str, cx: &App) -> Avatar {
    match image(user_id, cx) {
        Some(picture) => avatar.image(picture),
        None => avatar,
    }
}

/// An image from a picture's bytes, or `None` when they are not an image
/// GPUI can draw.
pub fn picture(bytes: &[u8], content_type: &str) -> Option<Arc<Image>> {
    let format = crate::live::image_format(bytes, content_type)?;
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    type Asked = Rc<RefCell<Vec<(String, Option<i64>)>>>;

    fn store() -> (Avatars, Asked) {
        let asked: Asked = Rc::default();
        let log = asked.clone();
        let avatars = Avatars::new(Rc::new(move |id: &str, version| {
            log.borrow_mut().push((id.to_owned(), version));
        }));
        (avatars, asked)
    }

    fn png() -> Arc<Image> {
        Arc::new(Image::from_bytes(
            gpui_kit::ImageFormat::Png,
            b"\x89PNG\r\n\x1a\n".to_vec(),
        ))
    }

    #[test]
    fn asks_once_per_user_and_version() {
        let (avatars, asked) = store();
        avatars.learn("u1", 10);
        assert!(asked.borrow().is_empty(), "learning alone fetches nothing");
        assert!(avatars.image("u1").is_none());
        assert!(avatars.image("u1").is_none());
        assert_eq!(*asked.borrow(), vec![("u1".to_owned(), Some(10))]);
        let picture = png();
        assert!(avatars.landed("u1", 10, Some(picture.clone())));
        assert!(Arc::ptr_eq(&avatars.image("u1").unwrap(), &picture));
        avatars.learn("u1", 10);
        assert_eq!(asked.borrow().len(), 1, "same version, no new fetch");
    }

    #[test]
    fn an_unknown_version_lets_sync_choose() {
        let (avatars, asked) = store();
        assert!(avatars.image("u1").is_none());
        assert_eq!(*asked.borrow(), vec![("u1".to_owned(), None)]);
        assert!(avatars.landed("u1", 7, Some(png())));
        // Learning the version sync used asks nothing more.
        avatars.learn("u1", 7);
        assert!(avatars.image("u1").is_some());
        assert_eq!(asked.borrow().len(), 1);
    }

    #[test]
    fn a_new_picture_is_fetched_and_replaces_the_old() {
        let (avatars, asked) = store();
        avatars.learn("u1", 10);
        avatars.image("u1");
        let old = png();
        avatars.landed("u1", 10, Some(old.clone()));
        // user_updated with a new picture time: asked at once, the old
        // picture stays up meanwhile.
        avatars.learn("u1", 20);
        assert_eq!(asked.borrow().last(), Some(&("u1".to_owned(), Some(20))));
        assert!(Arc::ptr_eq(&avatars.image("u1").unwrap(), &old));
        assert_eq!(asked.borrow().len(), 2, "not asked twice");
        let new = Arc::new(Image::from_bytes(
            gpui_kit::ImageFormat::Png,
            b"\x89PNG\r\n\x1a\nnew".to_vec(),
        ));
        assert!(avatars.landed("u1", 20, Some(new.clone())));
        assert!(Arc::ptr_eq(&avatars.image("u1").unwrap(), &new));
        // A late answer for the old version does not win.
        assert!(!avatars.landed("u1", 10, Some(old)));
        assert!(Arc::ptr_eq(&avatars.image("u1").unwrap(), &new));
    }

    #[test]
    fn a_change_for_someone_not_drawn_waits_for_a_view() {
        let (avatars, asked) = store();
        avatars.learn("u1", 10);
        avatars.learn("u1", 20);
        assert!(asked.borrow().is_empty());
        avatars.image("u1");
        assert_eq!(*asked.borrow(), vec![("u1".to_owned(), Some(20))]);
    }

    #[test]
    fn a_failure_keeps_the_initials_until_a_retry() {
        let (avatars, asked) = store();
        avatars.learn("u1", 10);
        avatars.image("u1");
        assert!(!avatars.landed("u1", 10, None));
        assert!(avatars.image("u1").is_none(), "initials stay");
        assert_eq!(
            asked.borrow().len(),
            1,
            "a failure is not retried per frame"
        );
        avatars.retry_failed();
        assert_eq!(asked.borrow().len(), 2);
        avatars.retry_failed();
        assert_eq!(asked.borrow().len(), 2, "only failures are retried");
        assert!(avatars.landed("u1", 10, Some(png())));
        assert!(avatars.image("u1").is_some());
    }

    #[test]
    fn without_sync_nothing_is_asked() {
        let avatars = Avatars::default();
        avatars.insert("u1", png());
        assert!(avatars.image("u1").is_some());
        assert!(avatars.image("u2").is_none());
        assert!(avatars.image("").is_none());
    }
}
