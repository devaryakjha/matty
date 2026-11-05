//! What installed Mattermost Apps add to a channel (#226): items in a
//! post's menu and buttons in the channel header, from the Apps plugin's
//! bindings.
//!
//! The channel view shows them from [`ChannelApps`], which reaches it with
//! the channel's details; choosing one emits [`AppAsk`], and the window
//! hands it to [`crate::live::Live::run_app`], which finds the binding and
//! asks sync to run it. A form opens as an interactive dialog, an App's
//! text shows as an ephemeral message, a link goes through Matty's link
//! routing, and a failure is a toast. A server without the Apps plugin has
//! no bindings, so nothing shows.

use std::collections::HashMap;

use gpui_kit::{App, EntityId, Global, SharedString, WeakEntity};
use matty_api::models::{AppBinding, location};

use crate::channel::{ChannelEvent, ChannelView};
use crate::ui::menu::MenuItem;

/// One binding as a menu item or button: its label, where it is (the full
/// location, which finds it again), and its sub-bindings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppItem {
    pub app_id: SharedString,
    pub label: SharedString,
    pub location: SharedString,
    pub description: SharedString,
    pub children: Vec<AppItem>,
}

/// A channel's App bindings, as the views show them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelApps {
    pub channel_id: SharedString,
    /// Items for each post's menu.
    pub post_menu: Vec<AppItem>,
    /// Buttons for the channel header.
    pub channel_header: Vec<AppItem>,
}

/// The user chose a binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAsk {
    pub channel_id: SharedString,
    /// The binding's full location, such as `/post_menu/send`.
    pub location: SharedString,
    /// The post, for a post menu item.
    pub post_id: Option<SharedString>,
    /// The post's thread root, when it is a reply.
    pub root_id: Option<SharedString>,
}

fn items(bindings: &[AppBinding], parent: &str, app_id: &str) -> Vec<AppItem> {
    bindings
        .iter()
        .filter(|binding| !binding.label.trim().is_empty() || !binding.bindings.is_empty())
        .map(|binding| {
            let location = format!("{parent}/{}", binding.name());
            let app_id = if binding.app_id.is_empty() {
                app_id
            } else {
                &binding.app_id
            };
            AppItem {
                app_id: app_id.to_owned().into(),
                label: binding.label.clone().into(),
                description: binding.description.clone().into(),
                children: items(&binding.bindings, &location, app_id),
                location: location.into(),
            }
        })
        .collect()
}

/// The views' bindings for a channel, from the server's top-level ones.
pub fn channel_apps(channel_id: &str, bindings: &[AppBinding]) -> ChannelApps {
    let at = |place: &str| {
        bindings
            .iter()
            .filter(|binding| binding.location == place)
            .flat_map(|binding| items(&binding.bindings, place, &binding.app_id))
            .collect::<Vec<_>>()
    };
    ChannelApps {
        channel_id: channel_id.to_owned().into(),
        post_menu: at(location::POST_MENU),
        channel_header: at(location::CHANNEL_HEADER),
    }
}

/// Finds the binding at `full` (as [`AppItem::location`] has it) and its
/// App, among the server's top-level bindings.
pub fn find<'a>(bindings: &'a [AppBinding], full: &str) -> Option<(&'a AppBinding, String)> {
    fn walk<'a>(
        list: &'a [AppBinding],
        parent: &str,
        app_id: &str,
        full: &str,
    ) -> Option<(&'a AppBinding, String)> {
        for binding in list {
            let here = format!("{parent}/{}", binding.name());
            let app = if binding.app_id.is_empty() {
                app_id
            } else {
                &binding.app_id
            };
            if here == full {
                return Some((binding, app.to_owned()));
            }
            if full.starts_with(&format!("{here}/"))
                && let Some(found) = walk(&binding.bindings, &here, app, full)
            {
                return Some(found);
            }
        }
        None
    }
    bindings.iter().find_map(|top| {
        full.starts_with(&format!("{}/", top.location))
            .then(|| walk(&top.bindings, &top.location, &top.app_id, full))
            .flatten()
    })
}

/// The bindings of every channel view, by the view, so a post's menu,
/// built while its row renders, can add the post menu ones.
#[derive(Default)]
struct PostMenus(HashMap<EntityId, ChannelApps>);

impl Global for PostMenus {}

/// Keeps a channel view's bindings for its posts' menus.
pub fn remember(view: EntityId, apps: &ChannelApps, cx: &mut App) {
    let menus = cx.default_global::<PostMenus>();
    if apps.post_menu.is_empty() {
        menus.0.remove(&view);
    } else {
        menus.0.insert(view, apps.clone());
    }
}

/// The view shows another channel: its bindings go until the new ones
/// come.
pub fn forget(view: EntityId, cx: &mut App) {
    if let Some(menus) = cx.try_global::<PostMenus>()
        && menus.0.contains_key(&view)
    {
        cx.global_mut::<PostMenus>().0.remove(&view);
    }
}

fn menu_items(
    items: &[AppItem],
    view: &WeakEntity<ChannelView>,
    ask: &AppAsk,
    prefix: &str,
) -> Vec<MenuItem> {
    items
        .iter()
        .map(|item| {
            let id = gpui_kit::ElementId::Name(format!("{prefix}{}", item.location).into());
            if !item.children.is_empty() {
                return MenuItem::submenu(
                    id,
                    item.label.clone(),
                    menu_items(&item.children, view, ask, prefix),
                );
            }
            let view = view.clone();
            let ask = AppAsk {
                location: item.location.clone(),
                ..ask.clone()
            };
            MenuItem::new(id, item.label.clone()).on_click(move |_, _, cx| {
                let event = ChannelEvent::App(ask.clone());
                view.update(cx, |_, cx| cx.emit(event)).ok();
            })
        })
        .collect()
}

/// The App items for a post's menu, after a separator; none when no App
/// adds any in the view's channel.
pub fn post_menu_items(
    view: &WeakEntity<ChannelView>,
    view_id: EntityId,
    post_id: &SharedString,
    root_id: Option<&SharedString>,
    cx: &App,
) -> Vec<MenuItem> {
    let Some(apps) = cx
        .try_global::<PostMenus>()
        .and_then(|menus| menus.0.get(&view_id))
    else {
        return Vec::new();
    };
    let ask = AppAsk {
        channel_id: apps.channel_id.clone(),
        location: SharedString::default(),
        post_id: Some(post_id.clone()),
        root_id: root_id.cloned(),
    };
    let mut out = vec![MenuItem::separator()];
    out.extend(menu_items(
        &apps.post_menu,
        view,
        &ask,
        &format!("post.menu.app.{post_id}"),
    ));
    out
}

/// The channel header's App items, for its Apps menu.
pub fn header_items(apps: &ChannelApps, view: &WeakEntity<ChannelView>) -> Vec<MenuItem> {
    let ask = AppAsk {
        channel_id: apps.channel_id.clone(),
        location: SharedString::default(),
        post_id: None,
        root_id: None,
    };
    menu_items(&apps.channel_header, view, &ask, "channel.apps.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bindings() -> Vec<AppBinding> {
        serde_json::from_value(json!([
            {"location": "/post_menu", "bindings": [
                {"app_id": "hello", "location": "send", "label": "Send hello",
                 "submit": {"path": "/send"}},
                {"app_id": "jira", "location": "jira", "label": "Jira", "bindings": [
                    {"location": "create", "label": "Create issue", "submit": {"path": "/c"}}
                ]}
            ]},
            {"location": "/channel_header", "bindings": [
                {"app_id": "hello", "label": "Wave", "submit": {"path": "/wave"}}
            ]},
            {"location": "/command", "bindings": [{"app_id": "hello", "label": "hello"}]}
        ]))
        .unwrap()
    }

    #[test]
    fn bindings_become_items_and_are_found_again() {
        let bindings = bindings();
        let apps = channel_apps("c", &bindings);
        let labels: Vec<_> = apps.post_menu.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Send hello", "Jira"]);
        let create = &apps.post_menu[1].children[0];
        assert_eq!(create.location.as_ref(), "/post_menu/jira/create");
        assert_eq!(create.app_id.as_ref(), "jira");
        assert_eq!(
            apps.channel_header[0].location.as_ref(),
            "/channel_header/Wave"
        );

        let (found, app) = find(&bindings, "/post_menu/jira/create").unwrap();
        assert_eq!(
            (found.label.as_str(), app.as_str()),
            ("Create issue", "jira")
        );
        let (found, _) = find(&bindings, "/channel_header/Wave").unwrap();
        assert_eq!(found.submit.as_ref().unwrap().path, "/wave");
        assert!(find(&bindings, "/post_menu/nope").is_none());
    }
}
