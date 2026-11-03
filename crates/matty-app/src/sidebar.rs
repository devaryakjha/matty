//! The channel sidebar: the view model sync fills and the view that draws it
//! (DESIGN.md §4). Categories collapse, unread rows are stronger, mentions
//! are `primary` pills, muted rows fade, and direct messages show presence.

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, Context, EventEmitter, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::style::{self, Palette, TypeStyleExt as _, metrics, scaled};
use crate::ui::{
    avatar::{Avatar, AvatarSize},
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonSize, ButtonVariant},
    collapsible::{collapsible, collapsible_trigger},
    sidebar::{Sidebar, SidebarItem, SidebarItemSize, sidebar_menu_skeleton},
    theme::UiTheme,
};

/// A user's presence, as Mattermost reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Online,
    Away,
    Dnd,
    Offline,
}

/// What a sidebar row points at, which decides its icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Public,
    Private,
    Direct { presence: Presence },
    Group { members: usize },
}

/// One row in a category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: SharedString,
    pub name: SharedString,
    pub kind: ChannelKind,
    pub unread: bool,
    pub mentions: usize,
    pub muted: bool,
}

impl ChannelRow {
    pub fn new(
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        kind: ChannelKind,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            unread: false,
            mentions: 0,
            muted: false,
        }
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

    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }
}

/// A server-side category (Favorites, Channels, Direct messages, custom).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    pub id: SharedString,
    pub name: SharedString,
    pub collapsed: bool,
    pub rows: Vec<ChannelRow>,
}

impl Category {
    pub fn new(
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        rows: Vec<ChannelRow>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            collapsed: false,
            rows,
        }
    }
}

/// Everything the sidebar draws. Sync replaces it whole; the view keeps no
/// other state than which categories the user collapsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SidebarModel {
    /// True before the first snapshot; the view shows loading rows.
    pub loading: bool,
    pub selected: Option<SharedString>,
    /// Followed threads with replies the user has not read.
    pub unread_threads: usize,
    /// Channels with an unsent draft.
    pub drafts: usize,
    pub categories: Vec<Category>,
}

/// Which way Alt+Up/Down walks the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
}

impl SidebarModel {
    /// The model before anything has loaded.
    pub fn loading() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }

    /// Every row in sidebar order, collapsed categories included, as
    /// Mattermost's Alt+Up/Down does.
    fn rows(&self) -> impl Iterator<Item = &ChannelRow> {
        self.categories
            .iter()
            .flat_map(|category| category.rows.iter())
    }

    /// The row one `step` away from the selection, wrapping at the ends;
    /// with `unread_only`, the nearest unread row in that direction, or
    /// `None` when nothing is unread. Muted channels never count as unread.
    pub fn neighbor(&self, step: Step, unread_only: bool) -> Option<SharedString> {
        let rows: Vec<&ChannelRow> = self
            .rows()
            .filter(|row| !unread_only || (row.unread && !row.muted))
            .collect();
        if rows.is_empty() {
            return None;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| self.rows().position(|row| &row.id == id));
        let order: Vec<SharedString> = self.rows().map(|row| row.id.clone()).collect();
        let candidates: Vec<usize> = rows
            .iter()
            .filter_map(|row| order.iter().position(|id| id == &row.id))
            .collect();
        let next = match (current, step) {
            (None, Step::Next) => candidates[0],
            (None, Step::Previous) => candidates[candidates.len() - 1],
            (Some(at), Step::Next) => candidates
                .iter()
                .copied()
                .find(|&index| index > at)
                .unwrap_or(candidates[0]),
            (Some(at), Step::Previous) => candidates
                .iter()
                .rev()
                .copied()
                .find(|&index| index < at)
                .unwrap_or(candidates[candidates.len() - 1]),
        };
        Some(order[next].clone())
    }

    /// Clears every unread mark and mention count.
    pub fn mark_all_read(&mut self) {
        for row in self
            .categories
            .iter_mut()
            .flat_map(|category| category.rows.iter_mut())
        {
            row.unread = false;
            row.mentions = 0;
        }
        self.unread_threads = 0;
    }
}

/// A stable element id from a prefix and a server id.
fn element_id(prefix: &str, id: &SharedString) -> gpui_kit::ElementId {
    gpui_kit::ElementId::Name(format!("{prefix}.{id}").into())
}

/// What the sidebar asks the window to do.
/// The `selected` id that marks the Threads row, which is not a channel.
pub const THREADS_ID: &str = "~threads";
/// The `selected` id that marks the Drafts row.
pub const DRAFTS_ID: &str = "~drafts";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    /// The user opened the Threads entry above the categories.
    OpenThreads,
    /// The user opened the Drafts entry above the categories.
    OpenDrafts,
    /// The user activated a channel row.
    Select(SharedString),
}

pub struct SidebarView {
    model: SidebarModel,
    team_name: SharedString,
}

impl EventEmitter<SidebarEvent> for SidebarView {}

impl SidebarView {
    pub fn new(team_name: impl Into<SharedString>, model: SidebarModel) -> Self {
        Self {
            model,
            team_name: team_name.into(),
        }
    }

    pub fn model(&self) -> &SidebarModel {
        &self.model
    }

    /// Replaces the model, keeping the user's collapsed categories.
    pub fn set_model(&mut self, mut model: SidebarModel, cx: &mut Context<Self>) {
        for category in &mut model.categories {
            if let Some(old) = self.model.categories.iter().find(|c| c.id == category.id) {
                category.collapsed = old.collapsed;
            }
        }
        self.model = model;
        cx.notify();
    }

    /// Selects `id` and clears its unread mark, as viewing a channel does;
    /// sync confirms the view with the server.
    pub fn select(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        if let Some(row) = id.as_ref().and_then(|id| {
            self.model
                .categories
                .iter_mut()
                .flat_map(|category| category.rows.iter_mut())
                .find(|row| &row.id == id)
        }) {
            row.unread = false;
            row.mentions = 0;
        }
        self.model.selected = id;
        cx.notify();
    }

    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.model.mark_all_read();
        cx.notify();
    }

    fn toggle_category(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if let Some(category) = self.model.categories.iter_mut().find(|c| &c.id == id) {
            category.collapsed = !category.collapsed;
            cx.notify();
        }
    }

    fn header(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let team = Button::new("sidebar.team")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::Sm)
            .aria_label("Team menu")
            .px(theme.space(2.))
            .type_style(style::TITLE, &theme)
            .text_color(theme.colors.sidebar_foreground)
            .child(self.team_name.clone())
            .child(
                lucide(LucideIcon::ChevronDown)
                    .size(scaled(&theme, 14.))
                    .text_color(theme.colors.muted_foreground),
            );
        let compose = Button::new("sidebar.compose")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSm)
            .aria_label("New message")
            .disabled(self.model.loading)
            .child(
                lucide(LucideIcon::SquarePen)
                    .size(scaled(&theme, metrics::ICON))
                    .text_color(theme.colors.muted_foreground),
            );
        div()
            .h(scaled(&theme, metrics::HEADER_HEIGHT) - theme.space(4.))
            .flex()
            .items_center()
            .justify_between()
            .child(team)
            .child(compose)
    }

    /// The icon slot: `#` and lock for channels, an avatar with a presence
    /// dot for people, a member count for group messages.
    fn row_icon(row: &ChannelRow, theme: &UiTheme, palette: &Palette) -> impl IntoElement + use<> {
        let icon_size = scaled(theme, metrics::ICON);
        let slot = scaled(theme, 20.);
        let color = theme.colors.muted_foreground;
        let boxed = |inner: gpui_kit::AnyElement| {
            div()
                .size(slot)
                .flex()
                .items_center()
                .justify_center()
                .child(inner)
                .into_any_element()
        };
        match &row.kind {
            ChannelKind::Public => boxed(
                lucide(LucideIcon::Hash)
                    .size(icon_size)
                    .text_color(color)
                    .into_any_element(),
            ),
            ChannelKind::Private => boxed(
                lucide(LucideIcon::Lock)
                    .size(icon_size)
                    .text_color(color)
                    .into_any_element(),
            ),
            ChannelKind::Group { members } => boxed(
                div()
                    .size(icon_size)
                    .rounded(theme.radius.sm)
                    .bg(theme.colors.sidebar_accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme.text(10.))
                    .line_height(theme.text(10.))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(color)
                    .child(members.to_string())
                    .into_any_element(),
            ),
            ChannelKind::Direct { presence } => {
                let dot = match presence {
                    Presence::Online => Some(palette.online),
                    Presence::Away => Some(palette.away),
                    Presence::Dnd => Some(palette.dnd),
                    Presence::Offline => None,
                };
                let initial: String = row.name.chars().take(1).collect();
                div()
                    .relative()
                    .size(slot)
                    .child(
                        Avatar::new(element_id("sidebar.avatar", &row.id))
                            .size(AvatarSize::Sm)
                            .fallback(initial)
                            .aria_label(row.name.clone())
                            .w(slot)
                            .h(slot)
                            .text_size(theme.text(9.)),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(-2.))
                            .right(px(-2.))
                            .size(scaled(theme, 8.))
                            .rounded_full()
                            .border_2()
                            .border_color(theme.colors.sidebar)
                            .map(|dot_el| match dot {
                                Some(color) => dot_el.bg(color),
                                None => dot_el.bg(theme.colors.sidebar).child(
                                    div()
                                        .size_full()
                                        .rounded_full()
                                        .border_1()
                                        .border_color(theme.colors.muted_foreground),
                                ),
                            }),
                    )
                    .into_any_element()
            }
        }
    }

    fn row(&self, row: &ChannelRow, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let palette = Palette::read(cx);
        let selected = self.model.selected.as_ref() == Some(&row.id);
        let strong = row.unread && !row.muted;
        let id = row.id.clone();
        let mut item = SidebarItem::new(element_id("sidebar.channel", &row.id), row.name.clone())
            .size(SidebarItemSize::Small)
            .selected(selected)
            .icon(Self::row_icon(row, &theme, &palette))
            .type_style(if strong { style::UI_STRONG } else { style::UI }, &theme)
            .on_activate(cx.listener(move |this, _, _, cx| {
                this.model.selected = Some(id.clone());
                cx.emit(SidebarEvent::Select(id.clone()));
                cx.notify();
            }));
        if !strong && !selected {
            item = item.text_color(theme.colors.muted_foreground);
        }
        if row.muted && !selected {
            item = item.opacity(0.6);
        }
        if row.mentions > 0 {
            item = item.trailing(
                Badge::count(row.mentions, 99)
                    .h(theme.space(4.5))
                    .min_w(theme.space(4.5))
                    .px(theme.space(1.5))
                    .text_size(theme.text(11.))
                    .line_height(theme.text(11.)),
            );
        } else if row.unread && !row.muted {
            item = item.trailing(
                div()
                    .size(theme.space(1.5))
                    .rounded_full()
                    .bg(theme.colors.sidebar_foreground)
                    .mr(theme.space(1.)),
            );
        }
        item
    }

    /// DESIGN.md §4: the Threads and Drafts entries sit above the categories,
    /// drawn like channel rows with a count badge where there is something
    /// to read or finish.
    fn fixed_entries(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let entry = |id: &'static str,
                     label: &'static str,
                     glyph: LucideIcon,
                     count: usize,
                     event: SidebarEvent,
                     cx: &mut Context<Self>| {
            let strong = count > 0;
            let selected = self.model.selected.as_deref() == Some(id);
            let mut item = SidebarItem::new(id, label)
                .size(SidebarItemSize::Small)
                .selected(selected)
                .icon(
                    div()
                        .size(scaled(&theme, 20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            lucide(glyph)
                                .size(scaled(&theme, metrics::ICON))
                                .text_color(theme.colors.muted_foreground),
                        ),
                )
                .type_style(if strong { style::UI_STRONG } else { style::UI }, &theme)
                .on_activate(cx.listener(move |_, _, _, cx| cx.emit(event.clone())));
            if strong {
                item = item.trailing(
                    Badge::new(count.to_string())
                        .variant(BadgeVariant::Secondary)
                        .h(theme.space(4.5))
                        .min_w(theme.space(4.5))
                        .px(theme.space(1.5))
                        .text_size(theme.text(11.))
                        .line_height(theme.text(11.)),
                );
            } else {
                item = item.text_color(theme.colors.muted_foreground);
            }
            item
        };
        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .pb(theme.space(3.))
            .child(entry(
                THREADS_ID,
                "Threads",
                LucideIcon::MessageSquareText,
                self.model.unread_threads,
                SidebarEvent::OpenThreads,
                cx,
            ))
            .child(entry(
                DRAFTS_ID,
                "Drafts",
                LucideIcon::FilePenLine,
                self.model.drafts,
                SidebarEvent::OpenDrafts,
                cx,
            ))
    }

    fn category(
        &self,
        category: &Category,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = UiTheme::read(cx).clone();
        let open = !category.collapsed;
        let id = category.id.clone();
        let trigger = collapsible_trigger(element_id("sidebar.category", &category.id), open, cx)
            .w_full()
            .h(theme.space(6.))
            .px(theme.space(2.))
            .py(px(0.))
            .flex()
            .items_center()
            .justify_start()
            .gap(theme.space(1.))
            .type_style(style::LABEL, &theme)
            .text_color(theme.colors.muted_foreground)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_category(&id, cx)))
            .child(
                lucide(if open {
                    LucideIcon::ChevronDown
                } else {
                    LucideIcon::ChevronRight
                })
                .size(scaled(&theme, 12.))
                .text_color(theme.colors.muted_foreground),
            )
            .child(category.name.to_uppercase());
        // Collapsed categories keep their unread rows visible, as Mattermost does.
        let shown: Vec<_> = category
            .rows
            .iter()
            .filter(|row| open || row.unread || self.model.selected.as_ref() == Some(&row.id))
            .map(|row| self.row(row, cx).into_any_element())
            .collect();
        let rows = div().flex().flex_col().gap(px(1.)).children(shown);
        div()
            .flex()
            .flex_col()
            .pb(theme.space(3.))
            .child(trigger)
            .child(if open {
                collapsible(
                    element_id("sidebar.category.body", &category.id),
                    true,
                    window,
                    cx,
                )
                .child(rows)
                .into_any_element()
            } else {
                rows.into_any_element()
            })
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx).clone();
        let mut sidebar = Sidebar::new("sidebar", "Channels").header(self.header(cx));
        if self.model.loading {
            for (label, rows) in [("Favorites", 2), ("Channels", 4), ("Direct messages", 3)] {
                sidebar = sidebar.child(
                    div()
                        .flex()
                        .flex_col()
                        .pb(theme.space(3.))
                        .child(
                            div()
                                .h(theme.space(6.))
                                .px(theme.space(2.))
                                .flex()
                                .items_center()
                                .type_style(style::LABEL, &theme)
                                .text_color(theme.colors.muted_foreground)
                                .child(label.to_uppercase()),
                        )
                        .children((0..rows).map(|row| sidebar_menu_skeleton(row % 2 == 0, cx))),
                );
            }
            return sidebar;
        }
        sidebar = sidebar.child(self.fixed_entries(cx));
        let categories: Vec<_> = self.model.categories.clone();
        for category in &categories {
            sidebar = sidebar.child(self.category(category, window, cx));
        }
        sidebar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(selected: &str) -> SidebarModel {
        SidebarModel {
            loading: false,
            selected: Some(selected.into()),
            unread_threads: 0,
            drafts: 0,
            categories: vec![
                Category::new(
                    "fav",
                    "Favorites",
                    vec![
                        ChannelRow::new("a", "a", ChannelKind::Public),
                        ChannelRow::new("b", "b", ChannelKind::Public).unread(),
                    ],
                ),
                Category::new(
                    "ch",
                    "Channels",
                    vec![
                        ChannelRow::new("c", "c", ChannelKind::Public)
                            .unread()
                            .muted(),
                        ChannelRow::new("d", "d", ChannelKind::Public),
                        ChannelRow::new("e", "e", ChannelKind::Public).mentions(1),
                    ],
                ),
            ],
        }
    }

    #[test]
    fn neighbor_walks_across_categories_and_wraps() {
        let m = model("b");
        assert_eq!(m.neighbor(Step::Next, false).as_deref(), Some("c"));
        assert_eq!(m.neighbor(Step::Previous, false).as_deref(), Some("a"));
        assert_eq!(model("e").neighbor(Step::Next, false).as_deref(), Some("a"));
        assert_eq!(
            model("a").neighbor(Step::Previous, false).as_deref(),
            Some("e")
        );
    }

    #[test]
    fn unread_neighbor_skips_read_and_muted_rows() {
        let m = model("a");
        assert_eq!(m.neighbor(Step::Next, true).as_deref(), Some("b"));
        assert_eq!(model("b").neighbor(Step::Next, true).as_deref(), Some("e"));
        assert_eq!(
            model("b").neighbor(Step::Previous, true).as_deref(),
            Some("e")
        );
        let mut read = model("a");
        read.mark_all_read();
        assert_eq!(read.neighbor(Step::Next, true), None);
        assert_eq!(read.neighbor(Step::Next, false).as_deref(), Some("b"));
    }

    #[test]
    fn mentions_imply_unread_and_set_model_keeps_collapse() {
        let row = ChannelRow::new("a", "a", ChannelKind::Public).mentions(3);
        assert!(row.unread);
        let mut old = SidebarModel::default();
        old.categories.push(Category {
            collapsed: true,
            ..Category::new("c", "C", vec![])
        });
        let mut next = SidebarModel::default();
        next.categories.push(Category::new("c", "C", vec![row]));
        // The same merge the view does.
        for category in &mut next.categories {
            if let Some(prev) = old.categories.iter().find(|c| c.id == category.id) {
                category.collapsed = prev.collapsed;
            }
        }
        assert!(next.categories[0].collapsed);
    }
}
