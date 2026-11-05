//! Keep running when closed (#634): while the Preferences switch is on,
//! closing the window leaves Matty running with an icon in the tray, whose
//! menu opens the window again or quits. On Linux the icon is a
//! StatusNotifierItem, which KDE, GNOME's AppIndicator extension and most
//! other panels show; on Windows it sits in the notification area. macOS
//! keeps Matty running in the dock anyway, so it has no tray and this
//! module is not built there.
//!
//! Without a tray to show the icon, Matty keeps quitting on close, so it
//! never runs with no way back to it.

use async_channel::Sender;

/// What the person picked from the tray icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Show the main window, opening it again when it was closed.
    Open,
    Quit,
}

/// The icon's width and height, in pixels.
const ICON_SIZE: u32 = 64;

/// The app icon (`packaging/linux/dev.arya.matty.svg`) at 64 pixels, as
/// RGBA with straight alpha.
const ICON_RGBA: &[u8] = include_bytes!("../assets/tray/icon-64.rgba");

/// The menu's entries, in order, with their labels.
const MENU: [(TrayEvent, &str); 2] = [
    (TrayEvent::Open, "Open Matty"),
    (TrayEvent::Quit, "Quit Matty"),
];

/// The icon as StatusNotifierItem wants it: ARGB32, each pixel in network
/// byte order.
#[cfg(target_os = "linux")]
pub fn argb(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, a]| [*a, *r, *g, *b])
        .collect()
}

/// The icon in the tray; dropping it takes the icon away.
pub struct Tray {
    _platform: platform::Handle,
}

/// Puts the icon in the tray. Picks from it arrive on `events`. Fails when
/// the desktop has nowhere to show it.
pub async fn show(events: Sender<TrayEvent>) -> Result<Tray, String> {
    platform::show(events)
        .await
        .map(|handle| Tray { _platform: handle })
}

#[cfg(target_os = "linux")]
mod platform {
    //! StatusNotifierItem (KDE's spec, which the freedesktop panels share)
    //! with a com.canonical.dbusmenu menu, both served on the session bus.

    use std::collections::HashMap;

    use async_channel::Sender;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    use super::{ICON_RGBA, ICON_SIZE, MENU, TrayEvent, argb};

    const ITEM_PATH: &str = "/StatusNotifierItem";
    const MENU_PATH: &str = "/MenuBar";

    /// The bus connection that serves the icon; closing it removes it.
    pub struct Handle {
        _connection: zbus::Connection,
    }

    pub async fn show(events: Sender<TrayEvent>) -> Result<Handle, String> {
        let connection = zbus::connection::Builder::session()
            .and_then(|builder| {
                builder.serve_at(
                    ITEM_PATH,
                    Item {
                        events: events.clone(),
                    },
                )
            })
            .and_then(|builder| builder.serve_at(MENU_PATH, Menu { events }))
            .map_err(|error| error.to_string())?
            .build()
            .await
            .map_err(|error| format!("no session bus: {error}"))?;
        // The item goes by the connection's unique name, which the panels
        // accept and which needs no name of its own, so Flatpak's sandbox
        // only has to let Matty talk to the watcher.
        let name = connection
            .unique_name()
            .map(ToString::to_string)
            .ok_or("no unique name on the session bus")?;
        connection
            .call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some("org.kde.StatusNotifierWatcher"),
                "RegisterStatusNotifierItem",
                &(name.as_str(),),
            )
            .await
            .map_err(|error| format!("no tray on this desktop: {error}"))?;
        Ok(Handle {
            _connection: connection,
        })
    }

    type Pixmap = (i32, i32, Vec<u8>);

    struct Item {
        events: Sender<TrayEvent>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl Item {
        #[zbus(property)]
        fn category(&self) -> &str {
            "Communications"
        }

        #[zbus(property)]
        fn id(&self) -> &str {
            crate::APP_ID
        }

        #[zbus(property)]
        fn title(&self) -> &str {
            "Matty"
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            "Active"
        }

        #[zbus(property)]
        fn icon_name(&self) -> &str {
            crate::APP_ID
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> Vec<Pixmap> {
            let size = ICON_SIZE as i32;
            vec![(size, size, argb(ICON_RGBA))]
        }

        #[zbus(property)]
        fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
            (String::new(), Vec::new(), "Matty".into(), String::new())
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn menu(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from(MENU_PATH).expect("a valid object path")
        }

        fn activate(&self, _x: i32, _y: i32) {
            let _ = self.events.try_send(TrayEvent::Open);
        }

        fn secondary_activate(&self, _x: i32, _y: i32) {
            let _ = self.events.try_send(TrayEvent::Open);
        }

        fn context_menu(&self, _x: i32, _y: i32) {}

        fn scroll(&self, _delta: i32, _orientation: &str) {}
    }

    struct Menu {
        events: Sender<TrayEvent>,
    }

    /// A menu entry's id: 0 is the root, entries count from 1.
    fn entry(id: i32) -> Option<(TrayEvent, &'static str)> {
        usize::try_from(id)
            .ok()
            .and_then(|id| id.checked_sub(1))
            .and_then(|index| MENU.get(index).copied())
    }

    fn properties(id: i32) -> HashMap<String, OwnedValue> {
        let mut properties = HashMap::new();
        if id == 0 {
            properties.insert("children-display".into(), owned(Value::from("submenu")));
        } else if let Some((_, label)) = entry(id) {
            properties.insert("label".into(), owned(Value::from(label)));
            properties.insert("enabled".into(), owned(Value::from(true)));
            properties.insert("visible".into(), owned(Value::from(true)));
        }
        properties
    }

    fn owned(value: Value<'_>) -> OwnedValue {
        value
            .try_to_owned()
            .expect("plain values have no file descriptors")
    }

    /// One menu node as dbusmenu's `(ia{sv}av)`; children come wrapped
    /// in variants.
    type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

    fn node(id: i32, children: Vec<OwnedValue>) -> Node {
        (id, properties(id), children)
    }

    fn ids() -> impl Iterator<Item = i32> {
        (1..=MENU.len()).map(|id| i32::try_from(id).unwrap_or(i32::MAX))
    }

    #[zbus::interface(name = "com.canonical.dbusmenu")]
    impl Menu {
        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }

        #[zbus(property)]
        fn text_direction(&self) -> &str {
            "ltr"
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            "normal"
        }

        #[zbus(property)]
        fn icon_theme_path(&self) -> Vec<String> {
            Vec::new()
        }

        fn get_layout(
            &self,
            parent_id: i32,
            _recursion_depth: i32,
            _property_names: Vec<String>,
        ) -> (u32, Node) {
            let children = if parent_id == 0 {
                ids()
                    .map(|id| owned(Value::from(node(id, Vec::new()))))
                    .collect()
            } else {
                Vec::new()
            };
            (1, node(parent_id, children))
        }

        fn get_group_properties(
            &self,
            ids: Vec<i32>,
            _property_names: Vec<String>,
        ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
            let ids = if ids.is_empty() {
                std::iter::once(0).chain(self::ids()).collect()
            } else {
                ids
            };
            ids.into_iter().map(|id| (id, properties(id))).collect()
        }

        fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
            properties(id)
                .remove(&name)
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no {name} on {id}")))
        }

        fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
            if event_id == "clicked"
                && let Some((event, _)) = entry(id)
            {
                let _ = self.events.try_send(event);
            }
        }

        fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
            for (id, event_id, data, timestamp) in events {
                self.event(id, event_id, data, timestamp);
            }
            Vec::new()
        }

        fn about_to_show(&self, _id: i32) -> bool {
            false
        }

        fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
            (Vec::new(), Vec::new())
        }

        #[zbus(signal)]
        async fn layout_updated(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            revision: u32,
            parent: i32,
        ) -> zbus::Result<()>;
    }
}

#[cfg(target_os = "windows")]
mod platform {
    //! The notification area icon, through `tray-icon`, whose window lives
    //! on the main thread's message loop.

    use async_channel::Sender;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use super::{ICON_RGBA, ICON_SIZE, MENU, TrayEvent};

    pub struct Handle {
        _icon: TrayIcon,
        _items: Vec<MenuItem>,
    }

    /// The menu item ids, fixed so the handlers, which `tray-icon` takes
    /// only once a run, still match after the switch goes off and on.
    fn item_id(index: usize) -> String {
        format!("matty-tray-{index}")
    }

    pub async fn show(events: Sender<TrayEvent>) -> Result<Handle, String> {
        let menu = Menu::new();
        let items: Vec<MenuItem> = MENU
            .iter()
            .enumerate()
            .map(|(index, (_, label))| MenuItem::with_id(item_id(index), *label, true, None))
            .collect();
        for item in &items {
            menu.append(item).map_err(|error| error.to_string())?;
        }
        let icon = Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE)
            .map_err(|error| error.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("Matty")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| error.to_string())?;
        let picked = events.clone();
        MenuEvent::set_event_handler(Some(move |menu_event: MenuEvent| {
            let event = MENU
                .iter()
                .enumerate()
                .find(|(index, _)| menu_event.id == item_id(*index))
                .map(|(_, (event, _))| *event);
            if let Some(event) = event {
                let _ = picked.try_send(event);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |icon_event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = icon_event
            {
                let _ = events.try_send(TrayEvent::Open);
            }
        }));
        Ok(Handle {
            _icon: tray,
            _items: items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_square_rgba() {
        assert_eq!(ICON_RGBA.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn argb_moves_alpha_first() {
        assert_eq!(
            argb(&[1, 2, 3, 4, 5, 6, 7, 8]),
            vec![4, 1, 2, 3, 8, 5, 6, 7]
        );
    }
}
