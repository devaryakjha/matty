//! The rail (DESIGN.md §4, PLAN §4.1): a 56px column that holds the saved
//! servers once there are two or more (#206), round and above a rule, and
//! the current server's teams once the user belongs to more than one, as
//! 32px squares. The current team is
//! filled with `primary` and marked on the rail's edge; an unread team wears
//! a dot and a team with mentions a count, both on the square's corner so
//! the rail reads at a glance like the sidebar.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    alert_dialog::{alert_dialog, alert_dialog_action, alert_dialog_cancel},
    badge::Badge,
    button::{Button, ButtonSize, ButtonVariant},
    dialog::{DialogHandle, dialog_description, dialog_popup, dialog_title},
    menu::{Menu, MenuItem, MenuState},
    separator::Separator,
    theme::UiTheme,
    tooltip::text_tooltip,
};

/// A saved server on the rail (#206).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// Its `SiteName`, or its host.
    pub name: SharedString,
}

impl Server {
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self { name: name.into() }
    }
}

/// Up to two initials from `name`'s words, as a square's label.
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect()
}

/// A team on the rail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Team {
    pub id: SharedString,
    pub name: SharedString,
    /// Some channel in the team is unread.
    pub unread: bool,
    /// Mentions of the user across the team's channels.
    pub mentions: usize,
    /// The team's `last_team_icon_update`; 0 when it has no icon, and the
    /// square shows initials.
    pub icon: i64,
}

impl Team {
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            unread: false,
            mentions: 0,
            icon: 0,
        }
    }

    /// The team's icon version, from its `last_team_icon_update`.
    pub fn icon(mut self, last_team_icon_update: i64) -> Self {
        self.icon = last_team_icon_update;
        self
    }

    /// The team's icon, once sync has it (#153). Learning the version here
    /// asks for a changed icon the first time the rail draws it.
    fn icon_image(&self, cx: &gpui_kit::App) -> Option<std::sync::Arc<gpui_kit::Image>> {
        if self.icon <= 0 {
            return None;
        }
        let icons = cx.try_global::<crate::avatars::TeamIcons>()?;
        icons.learn(&self.id, self.icon);
        icons.image(&self.id)
    }

    pub fn unread(mut self) -> Self {
        self.unread = true;
        self
    }

    pub fn mentions(mut self, count: usize) -> Self {
        self.mentions = count;
        self.unread |= count > 0;
        self
    }

    /// Up to two initials from the display name, as the square's label.
    pub fn initials(&self) -> String {
        initials(&self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RailEvent {
    /// The user picked a team; the view already shows it as current.
    Select(SharedString),
    /// The user pressed the dashed square to join another team.
    Join,
    /// The user picked the `n`th saved server; the window switches to it.
    SelectServer(usize),
    /// The user pressed the dashed circle to add a server.
    AddServer,
    /// The user confirmed removing the `n`th saved server and its data.
    RemoveServer(usize),
}

/// The rail's state and rendering. Sync fills the teams and their counts,
/// the window the saved servers; the window shows the rail only when
/// `is_multi` holds.
pub struct RailView {
    teams: Vec<Team>,
    current: Option<SharedString>,
    servers: Vec<Server>,
    current_server: Option<usize>,
    /// A server's right-click menu.
    server_menu: Entity<MenuState>,
    /// "Remove {server}?", for the server in `removing`.
    remove: DialogHandle,
    removing: Option<usize>,
}

impl EventEmitter<RailEvent> for RailView {}

impl RailView {
    pub fn new(teams: Vec<Team>, current: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            teams,
            current,
            servers: Vec::new(),
            current_server: None,
            server_menu: cx.new(|cx| MenuState::new([], cx)),
            remove: DialogHandle::new(false),
            removing: None,
        }
    }

    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    pub fn current_server(&self) -> Option<usize> {
        self.current_server
    }

    /// Replaces the saved servers and which one is shown.
    pub fn set_servers(
        &mut self,
        servers: Vec<Server>,
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.servers != servers || self.current_server != current {
            self.servers = servers;
            self.current_server = current;
            cx.notify();
        }
    }

    /// Whether there are two or more servers to switch between, which is
    /// when they show (#206).
    pub fn shows_servers(&self) -> bool {
        self.servers.len() > 1
    }

    fn shows_teams(&self) -> bool {
        self.teams.len() > 1
    }

    pub fn teams(&self) -> &[Team] {
        &self.teams
    }

    pub fn current(&self) -> Option<&SharedString> {
        self.current.as_ref()
    }

    /// Whether the user belongs to more than one team or has more than one
    /// server, which is when the rail shows (DESIGN.md §4, #206).
    pub fn is_multi(&self) -> bool {
        self.shows_teams() || self.shows_servers()
    }

    pub fn set_teams(
        &mut self,
        teams: Vec<Team>,
        current: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.teams = teams;
        self.current = current;
        cx.notify();
    }

    /// Updates one team's unread mark and mention count.
    pub fn set_unread(&mut self, id: &str, unread: bool, mentions: usize, cx: &mut Context<Self>) {
        if let Some(team) = self.teams.iter_mut().find(|team| team.id.as_ref() == id) {
            team.unread = unread || mentions > 0;
            team.mentions = mentions;
            cx.notify();
        }
    }

    /// Makes `id` current. The team's own counts stay until sync clears
    /// them channel by channel, as viewing one channel does not read the
    /// others.
    pub fn select(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if self.current.as_ref() == Some(&id) {
            return;
        }
        self.current = Some(id.clone());
        cx.emit(RailEvent::Select(id));
        cx.notify();
    }

    fn open_server_menu(
        &mut self,
        index: usize,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let items = [
            MenuItem::new("rail.server.remove", "Remove server…").on_click(move |_, window, cx| {
                view.update(cx, |this, cx| {
                    this.removing = Some(index);
                    this.remove.open(window, cx);
                    cx.notify();
                })
                .ok();
            }),
        ];
        self.server_menu.update(cx, |menu, cx| {
            menu.set_items(items, cx);
            menu.open_at(Some(at), window, cx);
        });
    }

    /// One round server button, with its place's Cmd/Ctrl+number in the
    /// tooltip and Remove on right-click.
    fn server(
        &self,
        index: usize,
        server: &Server,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let current = self.current_server == Some(index);
        let name = server.name.clone();
        let tip: SharedString = match index {
            0..=8 => {
                let key = index + 1;
                let keys =
                    crate::actions::format_keys(&format!("cmd-{key}"), &format!("ctrl-{key}"));
                format!("{name}  {keys}").into()
            }
            _ => name.clone(),
        };
        let button = Button::new(gpui_kit::ElementId::Name(
            format!("rail.server.{index}").into(),
        ))
        .variant(if current {
            ButtonVariant::Default
        } else {
            ButtonVariant::Ghost
        })
        .size(ButtonSize::Icon)
        .aria_label(name.clone())
        .rounded_full()
        .when(!current, |button| {
            button
                .bg(theme.colors.sidebar_accent)
                .text_color(theme.colors.sidebar_foreground)
        })
        .type_style(style::UI_STRONG, &theme)
        .child(initials(&name).to_uppercase())
        .on_click(cx.listener(move |this, _, _, cx| {
            if this.current_server != Some(index) {
                cx.emit(RailEvent::SelectServer(index));
            }
        }));
        div()
            .relative()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("rail.server.{index}.tip").into(),
                    ))
                    .tooltip(move |_, cx| text_tooltip(tip.clone(), cx))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Right,
                        cx.listener(move |this, event: &gpui_kit::MouseDownEvent, window, cx| {
                            this.open_server_menu(index, event.position, window, cx)
                        }),
                    )
                    .child(button),
            )
            .when(current, |cell| {
                cell.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(theme.space(1.))
                        .bottom(theme.space(1.))
                        .w(px(2.))
                        .rounded_r(px(2.))
                        .bg(theme.colors.primary),
                )
            })
    }

    /// "Remove {server}?": the server's sign-in and everything Matty keeps
    /// for it go.
    fn remove_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let name = self
            .removing
            .and_then(|index| self.servers.get(index))
            .map_or_else(
                || SharedString::from("this server"),
                |server| server.name.clone(),
            );
        let popup = dialog_popup("rail.remove.popup", "Remove server", cx)
            .child(dialog_title("rail.remove.title", cx).child(format!("Remove {name}?")))
            .child(dialog_description("rail.remove.description", cx).child(
                "This signs you out of the server and removes Matty's copy of its \
                     messages, files and drafts from this computer.",
            ))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.space(2.))
                    .pt(theme.space(2.))
                    .child(
                        alert_dialog_cancel("rail.remove.cancel")
                            .variant(ButtonVariant::Outline)
                            .label("Cancel"),
                    )
                    .child(
                        alert_dialog_action("rail.remove.confirm")
                            .variant(ButtonVariant::Destructive)
                            .label("Remove"),
                    ),
            );
        let view = cx.entity().downgrade();
        alert_dialog("rail.remove", &self.remove, popup, window, cx).on_ok(move |_, _, cx| {
            view.update(cx, |this, cx| {
                if let Some(index) = this.removing.take() {
                    cx.emit(RailEvent::RemoveServer(index));
                }
            })
            .ok();
            true
        })
    }

    fn square(
        &self,
        index: usize,
        team: &Team,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let current = self.current.as_ref() == Some(&team.id);
        let id = team.id.clone();
        let name = team.name.clone();
        let button = Button::new(gpui_kit::ElementId::Name(
            format!("rail.team.{index}").into(),
        ))
        // The current team is a primary button, so its hover stays primary;
        // the others are ghost buttons on the sidebar's accent, whose hover
        // is the accent color like the sidebar rows.
        .variant(if current {
            ButtonVariant::Default
        } else {
            ButtonVariant::Ghost
        })
        .size(ButtonSize::Icon)
        .aria_label(name.clone())
        .rounded(theme.radius.lg)
        .when(!current, |button| {
            button
                .bg(theme.colors.sidebar_accent)
                .text_color(theme.colors.sidebar_foreground)
        })
        .type_style(style::UI_STRONG, &theme)
        .child(match team.icon_image(cx) {
            Some(icon) => gpui_kit::img(icon)
                .size_full()
                .rounded(theme.radius.lg)
                .object_fit(gpui_kit::ObjectFit::Cover)
                .into_any_element(),
            None => team.initials().into_any_element(),
        })
        .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)));
        // The badge sits on the square's top-right corner with a ring in the
        // rail's color so it stands off the tile; the ring also keeps the
        // badge from touching the next square.
        let corner = |cell: gpui_kit::Div| {
            cell.absolute()
                .top(px(-4.))
                .right(px(-4.))
                .border_2()
                .border_color(theme.colors.sidebar)
                .rounded_full()
        };
        let tip = name.clone();
        div()
            .relative()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("rail.team.{index}.tip").into(),
                    ))
                    .relative()
                    .tooltip(move |_, cx| text_tooltip(tip.clone(), cx))
                    .child(button)
                    .when(team.mentions > 0, |cell| {
                        cell.child(
                            corner(div()).child(
                                Badge::count(team.mentions, 99)
                                    .h(theme.space(4.))
                                    .min_w(theme.space(4.))
                                    .px(theme.space(1.))
                                    .text_size(theme.text(10.))
                                    .line_height(theme.text(10.)),
                            ),
                        )
                    })
                    .when(team.mentions == 0 && team.unread, |cell| {
                        cell.child(
                            corner(div())
                                .w(theme.space(3.))
                                .h(theme.space(3.))
                                .flex_shrink_0()
                                .bg(theme.colors.sidebar_foreground),
                        )
                    }),
            )
            .when(current, |cell| {
                cell.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(theme.space(1.))
                        .bottom(theme.space(1.))
                        .w(px(2.))
                        .rounded_r(px(2.))
                        .bg(theme.colors.primary),
                )
            })
    }
}

impl Render for RailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let servers: Vec<_> = if self.shows_servers() {
            self.servers
                .clone()
                .iter()
                .enumerate()
                .map(|(index, server)| self.server(index, server, cx).into_any_element())
                .collect()
        } else {
            Vec::new()
        };
        let squares: Vec<_> = if self.shows_teams() {
            self.teams
                .clone()
                .iter()
                .enumerate()
                .map(|(index, team)| self.square(index, team, cx).into_any_element())
                .collect()
        } else {
            Vec::new()
        };
        let shows_servers = self.shows_servers();
        let shows_teams = self.shows_teams();
        div()
            .w(scaled(&theme, metrics::RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(theme.space(2.))
            .py(theme.space(2.))
            .bg(theme.colors.sidebar)
            .border_r(style::hairline())
            .border_color(theme.colors.sidebar_border)
            .children(servers)
            .when(shows_servers, |rail| {
                rail.child(
                    div()
                        .id("rail.add-server.tip")
                        .tooltip(move |_, cx| text_tooltip("Add a server".into(), cx))
                        .child(
                            Button::new("rail.add-server")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Icon)
                                .aria_label("Add a server")
                                .rounded_full()
                                .border_1()
                                .border_dashed()
                                .border_color(theme.colors.border)
                                .child(
                                    lucide(LucideIcon::Plus)
                                        .size(scaled(&theme, metrics::ICON))
                                        .text_color(theme.colors.muted_foreground),
                                )
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(RailEvent::AddServer))),
                        ),
                )
            })
            .when(shows_servers && shows_teams, |rail| {
                rail.child(
                    Separator::new("rail.servers.rule")
                        .horizontal()
                        .w(scaled(&theme, 32.)),
                )
            })
            .children(squares)
            .when(shows_teams, |rail| {
                rail.child(
                    div()
                        .id("rail.add.tip")
                        .tooltip(move |_, cx| text_tooltip("Join another team".into(), cx))
                        .child(
                            Button::new("rail.add")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Icon)
                                .aria_label("Join another team")
                                .rounded(theme.radius.lg)
                                .border_1()
                                .border_dashed()
                                .border_color(theme.colors.border)
                                .child(
                                    lucide(LucideIcon::Plus)
                                        .size(scaled(&theme, metrics::ICON))
                                        .text_color(theme.colors.muted_foreground),
                                )
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(RailEvent::Join))),
                        ),
                )
            })
            .child(
                Menu::new(&self.server_menu, "Server options")
                    .context_area(div())
                    .disabled(true),
            )
            .child(self.remove_dialog(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_two_words() {
        assert_eq!(Team::new("a", "Matty").initials(), "M");
        assert_eq!(Team::new("b", "Design Guild").initials(), "DG");
        assert_eq!(Team::new("c", "Open Source Friends").initials(), "OS");
    }

    #[gpui_kit::test]
    fn servers_show_from_two_and_teams_from_two(cx: &mut gpui_kit::TestAppContext) {
        let rail = cx.new(|cx| RailView::new(vec![Team::new("a", "Matty")], None, cx));
        rail.read_with(cx, |rail, _| assert!(!rail.is_multi()));
        rail.update(cx, |rail, cx| {
            rail.set_servers(vec![Server::new("Acme")], Some(0), cx)
        });
        rail.read_with(cx, |rail, _| assert!(!rail.is_multi()));
        rail.update(cx, |rail, cx| {
            rail.set_servers(
                vec![Server::new("Acme"), Server::new("Matty Community")],
                Some(1),
                cx,
            )
        });
        rail.read_with(cx, |rail, _| {
            assert!(rail.is_multi() && rail.shows_servers() && !rail.shows_teams());
            assert_eq!(rail.current_server(), Some(1));
        });
        assert_eq!(initials("Matty Community"), "MC");
    }

    #[test]
    fn mentions_imply_unread() {
        let team = Team::new("a", "Matty").mentions(2);
        assert!(team.unread);
        assert_eq!(team.mentions, 2);
        assert!(!Team::new("b", "Quiet").unread);
    }
}
