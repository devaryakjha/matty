//! Images messages show from outside the server (#159, PLAN §6.6): the
//! pictures of link previews and attachments, and inline Markdown images.
//!
//! Sync fetches each one through the server's image proxy when the server
//! has one, and directly otherwise, as the webapp does, and keeps it in the
//! media cache. With "Load remote images: Ask" in Preferences nothing is
//! fetched until the person clicks the image's placeholder.
//!
//! Views read [`RemoteImages`] through the app global while they render, as
//! they do avatars; [`crate::live::Live`] fills it as images land.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Global, Image, ImageSource, SharedString};

/// Sends sync the request for one image.
pub type Fetch = Rc<dyn Fn(&str)>;

/// The outside images known so far, shared between Live, which receives
/// them, and the views. Everything runs on the main thread.
#[derive(Clone, Default)]
pub struct RemoteImages(Rc<RefCell<Store>>);

impl Global for RemoteImages {}

#[derive(Default)]
struct Store {
    images: HashMap<String, Arc<Image>>,
    /// Asked of sync, so each image is fetched once.
    asked: HashSet<String>,
    /// "Load remote images: Ask": only a click fetches.
    ask_first: bool,
    fetch: Option<Fetch>,
}

impl RemoteImages {
    /// A store that asks sync through `fetch`.
    pub fn new(fetch: Fetch, ask_first: bool) -> Self {
        Self(Rc::new(RefCell::new(Store {
            ask_first,
            fetch: Some(fetch),
            ..Store::default()
        })))
    }

    /// The image at `url`, once it has landed.
    pub fn image(&self, url: &str) -> Option<Arc<Image>> {
        self.0.borrow().images.get(url).cloned()
    }

    /// Whether `url` waits for the person to load it: Ask is on and the
    /// image is neither here nor asked for.
    pub fn waiting(&self, url: &str) -> bool {
        let store = self.0.borrow();
        store.ask_first && !store.images.contains_key(url) && !store.asked.contains(url)
    }

    /// A view shows `url`: fetch it unless it is here, already asked for,
    /// or waiting for a click.
    pub fn want(&self, url: &str) {
        if !self.0.borrow().ask_first {
            self.fetch(url);
        }
    }

    /// The person clicked `url`'s placeholder: fetch it whatever the
    /// setting.
    pub fn load(&self, url: &str) {
        self.fetch(url);
    }

    fn fetch(&self, url: &str) {
        if !is_web(url) {
            return;
        }
        let fetch = {
            let mut store = self.0.borrow_mut();
            if store.images.contains_key(url) || !store.asked.insert(url.to_owned()) {
                return;
            }
            store.fetch.clone()
        };
        if let Some(fetch) = fetch {
            fetch(url);
        }
    }

    /// Whether sync was asked for `url`.
    #[cfg(test)]
    pub fn asked(&self, url: &str) -> bool {
        self.0.borrow().asked.contains(url)
    }

    /// `url`'s image arrived.
    pub fn landed(&self, url: &str, image: Arc<Image>) {
        self.0.borrow_mut().images.insert(url.to_owned(), image);
    }

    /// Follows the Preferences setting. Returns whether it changed.
    pub fn set_ask_first(&self, ask_first: bool) -> bool {
        let mut store = self.0.borrow_mut();
        let changed = store.ask_first != ask_first;
        store.ask_first = ask_first;
        changed
    }
}

/// Only web images are fetched; anything else stays a placeholder.
fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn store(cx: &App) -> Option<&RemoteImages> {
    cx.try_global::<RemoteImages>()
}

/// An inline Markdown image for gpuicn's `MarkdownImages`: the image once
/// it has landed; until then `None`, which draws the alt text in a muted
/// frame, and a fetch unless the person is asked first.
pub fn source(url: &str, cx: &App) -> Option<ImageSource> {
    let store = store(cx)?;
    match store.image(url) {
        Some(image) => Some(ImageSource::Image(image)),
        None => {
            store.want(url);
            None
        }
    }
}

/// Whether `url` shows a click-to-load placeholder.
pub fn waiting(url: &str, cx: &App) -> bool {
    store(cx).is_some_and(|store| store.waiting(url))
}

/// Whether `url` has landed.
pub fn loaded(url: &str, cx: &App) -> bool {
    store(cx).is_some_and(|store| store.image(url).is_some())
}

/// The person asked to load `url`: its placeholder gives way to the
/// image's empty box until the image lands.
pub fn load(url: &str, cx: &mut App) {
    if let Some(store) = store(cx) {
        store.load(url);
    }
    cx.refresh_windows();
}

/// The host a placeholder names, such as "github.com".
pub fn host(url: &str) -> SharedString {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default()
        .into()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn store(ask_first: bool) -> (RemoteImages, Rc<RefCell<Vec<String>>>) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let log = sent.clone();
        let images = RemoteImages::new(
            Rc::new(move |url: &str| log.borrow_mut().push(url.to_owned())),
            ask_first,
        );
        (images, sent)
    }

    fn png() -> Arc<Image> {
        crate::attachments::png(b"\x89PNG\r\n\x1a\n")
    }

    #[test]
    fn always_fetches_each_image_once() {
        let (images, sent) = store(false);
        assert!(!images.waiting("https://a.example/x.png"));
        images.want("https://a.example/x.png");
        images.want("https://a.example/x.png");
        assert_eq!(*sent.borrow(), ["https://a.example/x.png"]);
        images.landed("https://a.example/x.png", png());
        assert!(images.image("https://a.example/x.png").is_some());
    }

    #[test]
    fn ask_waits_for_a_click() {
        let (images, sent) = store(true);
        let url = "https://a.example/x.png";
        images.want(url);
        assert!(sent.borrow().is_empty(), "nothing loads on its own");
        assert!(images.waiting(url));
        images.load(url);
        images.load(url);
        assert_eq!(*sent.borrow(), [url]);
        assert!(!images.waiting(url), "loading, so no placeholder");
        images.landed(url, png());
        assert!(!images.waiting(url));
    }

    #[test]
    fn switching_to_always_fetches_what_was_waiting() {
        let (images, sent) = store(true);
        images.want("https://a.example/x.png");
        assert!(images.set_ask_first(false));
        assert!(!images.set_ask_first(false));
        images.want("https://a.example/x.png");
        assert_eq!(sent.borrow().len(), 1);
    }

    #[test]
    fn only_web_images_are_fetched() {
        let (images, sent) = store(false);
        for url in ["emoji:tada", "data:image/png;base64,AA", "javascript:x", ""] {
            images.want(url);
            images.load(url);
        }
        assert!(sent.borrow().is_empty());
    }

    #[test]
    fn the_placeholder_names_the_host() {
        assert_eq!(
            host("https://cdn.example.org:8443/a.png?x"),
            "cdn.example.org"
        );
        assert_eq!(host("nope"), "");
    }
}
