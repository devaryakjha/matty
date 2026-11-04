//! The team rail (DESIGN.md §4): a 56px column of 32px team squares that
//! appears once the user belongs to more than one team. The current team is
//! filled with `primary` and marked on the rail's edge; an unread team wears
//! a dot and a team with mentions a count, both on the square's corner so
//! the rail reads at a glance like the sidebar.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::style::{self, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    badge::Badge,
    button::{Button, ButtonSize, ButtonVariant},
    theme::UiTheme,
    tooltip::text_tooltip,
};

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
        self.name
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .take(2)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RailEvent {
    /// The user picked a team; the view already shows it as current.
    Select(SharedString),
    /// The user pressed the dashed square to join another team.
    Join,
}

/// The rail's state and rendering. Sync fills the teams and their counts;
/// the window shows the rail only when `is_multi` holds.
pub struct RailView {
    teams: Vec<Team>,
    current: Option<SharedString>,
}

impl EventEmitter<RailEvent> for RailView {}

impl RailView {
    pub fn new(teams: Vec<Team>, current: Option<SharedString>) -> Self {
        Self { teams, current }
    }

    pub fn teams(&self) -> &[Team] {
        &self.teams
    }

    pub fn current(&self) -> Option<&SharedString> {
        self.current.as_ref()
    }

    /// Whether the user belongs to more than one team, which is when the
    /// rail shows (DESIGN.md §4).
    pub fn is_multi(&self) -> bool {
        self.teams.len() > 1
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let squares: Vec<_> = self
            .teams
            .clone()
            .iter()
            .enumerate()
            .map(|(index, team)| self.square(index, team, cx).into_any_element())
            .collect();
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
            .children(squares)
            .child(
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

    #[test]
    fn mentions_imply_unread() {
        let team = Team::new("a", "Matty").mentions(2);
        assert!(team.unread);
        assert_eq!(team.mentions, 2);
        assert!(!Team::new("b", "Quiet").unread);
    }
}
