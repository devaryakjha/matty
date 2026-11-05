//! Matty's design tokens on top of gpuicn's [`UiTheme`] (docs/DESIGN.md).
//!
//! The color theme is `assets/theme.css` in shadcn's variable format, so it
//! can be tuned in tweakcn and pasted back. This module turns it into a
//! [`UiTheme`] per mode, adds the semantic colors shadcn has no name for
//! ([`Palette`]), and names the type scale and layout metrics every view
//! shares, so a size appears once here and nowhere else.

use gpui_kit::{
    App, ElementId, FontWeight, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Rgba, SharedString, StatefulInteractiveElement as _, Styled, div,
    prelude::FluentBuilder as _, px, rgb,
};

use gpui_icons::{LucideIcon, lucide};

use crate::ui::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use crate::ui::theme::{ThemeMode, UiTheme};
use crate::ui::tooltip::text_tooltip;

const THEME_CSS: &str = include_str!("../assets/theme.css");

/// Builds Matty's theme for `mode` from `assets/theme.css`.
///
/// The CSS is checked by a test, so a parse failure here is a build mistake;
/// the app still opens, on gpuicn's Neutral palette, rather than panicking.
pub fn theme(mode: ThemeMode) -> UiTheme {
    let mut theme = match UiTheme::from_shadcn_css(THEME_CSS, mode) {
        Ok(theme) => theme,
        Err(error) => {
            eprintln!("matty: theme.css did not parse ({error:?}); using the Neutral palette");
            match mode {
                ThemeMode::Light => UiTheme::neutral_light(),
                ThemeMode::Dark => UiTheme::neutral_dark(),
            }
        }
    };
    // Matty's sidebar reads quiet text and mention counts like the channel
    // view does; a Mattermost theme (#214) colors them apart.
    let c = &mut theme.colors;
    c.sidebar_muted_foreground = Some(c.muted_foreground);
    c.sidebar_badge = Some(c.primary);
    c.sidebar_badge_foreground = Some(c.primary_foreground);
    theme
}

/// Colors with a meaning gpuicn's tokens do not cover (DESIGN.md §2):
/// presence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Presence dot: online.
    pub online: Rgba,
    /// Presence dot: away.
    pub away: Rgba,
    /// Presence dot: do not disturb.
    pub dnd: Rgba,
}

impl Global for Palette {}

impl Palette {
    /// The semantic colors for `mode`.
    pub fn for_mode(mode: ThemeMode) -> Self {
        let (online, away, dnd) = match mode {
            ThemeMode::Light => (rgb(0x16a34a), rgb(0xcc7a00), rgb(0xd42020)),
            ThemeMode::Dark => (rgb(0x3fb950), rgb(0xf2b038), rgb(0xf0564f)),
        };
        Self { online, away, dnd }
    }

    /// The palette for the current theme mode. Falls back to light before
    /// [`install`] has run.
    pub fn read(cx: &App) -> Self {
        cx.try_global::<Self>()
            .copied()
            .unwrap_or_else(|| Self::for_mode(ThemeMode::Light))
    }
}

/// Installs Matty's theme for `mode`, keeping the text scale, spacing unit
/// and motion settings the app already applied. Does nothing when the
/// installed theme is already this one, so views are not repainted.
pub fn install(mode: ThemeMode, cx: &mut App) {
    install_theme(theme(mode), Palette::for_mode(mode), cx);
}

/// Installs `next` and its `palette`, such as an imported Mattermost theme
/// (#214), keeping the text scale, spacing unit and motion settings the app
/// already applied. Does nothing when both are already installed.
pub fn install_theme(mut next: UiTheme, palette: Palette, cx: &mut App) {
    if cx.has_global::<UiTheme>() {
        let current = UiTheme::read(cx);
        next.text_scale = current.text_scale;
        next.spacing = current.spacing;
        next.motion = current.motion;
        if *current == next && cx.try_global::<Palette>() == Some(&palette) {
            return;
        }
    }
    cx.set_global(palette);
    UiTheme::set(cx, next);
}

/// Applies the "Reduce motion" preference to the installed theme, so every
/// gpuicn transition snaps to its target; a no-op when nothing changes.
pub fn set_reduced_motion(reduced: bool, cx: &mut App) {
    let theme = UiTheme::read(cx);
    if theme.motion.reduced != reduced {
        let mut next = theme.clone();
        next.motion.reduced = reduced;
        UiTheme::set(cx, next);
    }
}

/// A role in the type scale (DESIGN.md §3): size and line height in px at
/// text scale 1, and the weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    /// Font size at text scale 1.
    pub size: f32,
    /// Line height at text scale 1.
    pub line: f32,
    /// Font weight.
    pub weight: FontWeight,
}

/// Channel names in headers and dialog titles.
pub const TITLE: TypeStyle = TypeStyle {
    size: 15.,
    line: 20.,
    weight: FontWeight::MEDIUM,
};
/// Post text and the composer.
pub const BODY: TypeStyle = TypeStyle {
    size: 14.,
    line: 21.,
    weight: FontWeight::NORMAL,
};
/// Sidebar rows, buttons, menus and the title bar.
pub const UI: TypeStyle = TypeStyle {
    size: 13.,
    line: 18.,
    weight: FontWeight::NORMAL,
};
/// Unread rows, author names and labels.
pub const UI_STRONG: TypeStyle = TypeStyle {
    size: 13.,
    line: 18.,
    weight: FontWeight::MEDIUM,
};
/// Timestamps, counts and descriptions.
pub const META: TypeStyle = TypeStyle {
    size: 12.,
    line: 16.,
    weight: FontWeight::NORMAL,
};
/// Sidebar category names; render them uppercase with [`LABEL_TRACKING`].
pub const LABEL: TypeStyle = TypeStyle {
    size: 11.,
    line: 16.,
    weight: FontWeight::MEDIUM,
};
/// Inline code and code blocks, in the mono font.
pub const CODE: TypeStyle = TypeStyle {
    size: 13.,
    line: 20.,
    weight: FontWeight::NORMAL,
};
/// Letter spacing for [`LABEL`], in px at text scale 1.
pub const LABEL_TRACKING: f32 = 0.4;

/// Applies a [`TypeStyle`] to any styled element, scaled by the theme.
pub trait TypeStyleExt: Styled + Sized {
    /// Sets font size, line height and weight from `style`.
    fn type_style(self, style: TypeStyle, theme: &UiTheme) -> Self {
        self.text_size(theme.text(style.size))
            .line_height(theme.text(style.line))
            .font_weight(style.weight)
    }
}

impl<T: Styled> TypeStyleExt for T {}

/// Layout metrics from DESIGN.md §4, in px at zoom 1. Callers multiply by
/// the theme's spacing unit through [`scaled`] so zoom applies.
pub mod metrics {
    /// The team rail's width.
    pub const RAIL_WIDTH: f32 = 56.;
    /// Sidebar, channel and right-pane headers.
    pub const HEADER_HEIGHT: f32 = 48.;
    /// A sidebar row.
    pub const ROW_HEIGHT: f32 = 28.;
    /// Horizontal padding inside the channel view.
    pub const GUTTER: f32 = 20.;
    /// The quick switcher pill's widest size in the title bar.
    pub const SWITCHER_MAX_WIDTH: f32 = 560.;
    /// Chrome icons.
    pub const ICON: f32 = 16.;
    /// Icons in empty states.
    pub const ICON_LARGE: f32 = 20.;
    /// Icons inside chips, badges, menu rows and the channel header's
    /// secondary controls (DESIGN.md §5).
    pub const ICON_SMALL: f32 = 14.;
    /// Chevrons and kind glyphs beside text.
    pub const ICON_TINY: f32 = 12.;
    /// The square an icon or initial sits in on a sidebar row.
    pub const ROW_SLOT: f32 = 20.;
    /// The "… is typing" line under the transcript.
    pub const TYPING_LINE: f32 = 20.;
    /// The widest an empty state's text runs.
    pub const EMPTY_MAX_WIDTH: f32 = 420.;
    /// The Threads view's list column.
    pub const THREADS_LIST_WIDTH: f32 = 360.;
    /// The widest a Drafts card runs.
    pub const DRAFTS_MAX_WIDTH: f32 = 760.;
    /// A short field such as a time picker.
    pub const FIELD_NARROW_WIDTH: f32 = 140.;
    /// Post avatars in the channel view and threads.
    pub const AVATAR: f32 = 32.;
}

/// Scales a px metric by the theme's spacing unit (4px at zoom 1).
pub fn scaled(theme: &UiTheme, pixels: f32) -> Pixels {
    theme.spacing.unit * (pixels / 4.)
}

/// Returns `pixels` as [`Pixels`] unscaled, for one-pixel hairlines.
pub const fn hairline() -> Pixels {
    px(1.)
}

/// An empty state the way DESIGN.md §3 draws every one: a 40px icon in a
/// tinted circle, a `title` line and one `meta` sentence, all centered.
/// Callers add at most one outline button through [`Empty::content`].
pub fn empty_state(
    glyph: LucideIcon,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cx: &App,
) -> Empty {
    let theme = UiTheme::read(cx);
    Empty::new().header(
        EmptyHeader::new()
            .media(
                EmptyMedia::new()
                    .with_variant(EmptyMediaVariant::Icon)
                    .size(theme.space(10.))
                    .rounded_full()
                    .child(
                        lucide(glyph)
                            .size(scaled(theme, metrics::ICON_LARGE))
                            .text_color(theme.colors.muted_foreground),
                    ),
            )
            .title(
                EmptyTitle::new()
                    .type_style(TITLE, theme)
                    .text_center()
                    .child(title.into()),
            )
            .description(
                EmptyDescription::new()
                    .type_style(META, theme)
                    .text_center()
                    .child(description.into()),
            ),
    )
}

/// [`empty_state`] centered in the space it fills, for a pane with nothing
/// else to show.
pub fn centered_empty(
    glyph: LucideIcon,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cx: &App,
) -> gpui_kit::Div {
    let theme = UiTheme::read(cx);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p(theme.space(6.))
        .child(empty_state(glyph, title, description, cx))
}

/// Wraps `child` so hovering it shows `tip` (DESIGN.md §5: every icon-only
/// button has a tooltip, with its shortcut when there is one). `id` must be
/// unique within the window; the wrapper sizes to its child.
pub fn tipped(
    id: impl Into<ElementId>,
    tip: impl Into<SharedString>,
    child: impl IntoElement,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let tip: SharedString = tip.into();
    tipped_when(true, id, tip, child)
}

/// [`tipped`] whose tooltip is attached only while `show` is true. Pass
/// `false` while a menu or popover opened from the control is showing, so
/// the tooltip does not linger over it.
pub fn tipped_when(
    show: bool,
    id: impl Into<ElementId>,
    tip: impl Into<SharedString>,
    child: impl IntoElement,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let tip: SharedString = tip.into();
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .when(show, |this| {
            this.tooltip(move |_, cx| text_tooltip(tip.clone(), cx))
        })
        .child(child)
}

/// The tallest a composer's text box grows before it scrolls: 40% of the
/// window, read on every render so it follows resizes.
pub fn composer_cap(window: &gpui_kit::Window) -> gpui_kit::Pixels {
    window.viewport_size().height * 0.4
}

/// The presence dot over an avatar's bottom-right corner (DESIGN.md §3,
/// #135): filled for online, away and Do not disturb, a hollow ring for
/// offline. `ring` is the surface it sits on, so the dot reads as cut out
/// of the avatar. The dot has no label of its own; the avatar's label says
/// the presence (see [`presence_name`]).
///
/// Only for the 18px avatars in the sidebar and switcher: gpuicn's
/// `Avatar::status` sizes its dot for a 24px avatar, too big there
/// (devaryakjha/gpuicn#127). Every other avatar uses [`avatar_status`].
pub fn presence_dot(
    presence: crate::sidebar::Presence,
    size: Pixels,
    ring: Rgba,
    theme: &UiTheme,
    palette: &Palette,
) -> gpui_kit::Div {
    use crate::sidebar::Presence;
    let dot = gpui_kit::div()
        .absolute()
        .bottom(gpui_kit::px(-2.))
        .right(gpui_kit::px(-2.))
        .size(size)
        .rounded_full()
        .border_2()
        .border_color(ring);
    let fill = match presence {
        Presence::Online => palette.online,
        Presence::Away => palette.away,
        Presence::Dnd => palette.dnd,
        Presence::Offline => {
            // On the sidebar, the sidebar's own quiet color (#214).
            let color = if ring == theme.colors.sidebar {
                theme.colors.sidebar_muted_foreground()
            } else {
                theme.colors.muted_foreground
            };
            return dot.bg(ring).child(
                gpui_kit::div()
                    .size_full()
                    .rounded_full()
                    .border_1()
                    .border_color(color),
            );
        }
    };
    dot.bg(fill)
}

/// An avatar's accessible label with the presence its dot shows, such as
/// "Ana Lopez, away".
pub fn presence_name(name: &str, presence: crate::sidebar::Presence) -> SharedString {
    let state = crate::account::presence_label(presence).to_lowercase();
    format!("{name}, {state}").into()
}

/// The presence an avatar shows (DESIGN.md §3, #135) as gpuicn's avatar
/// status: a dot over the bottom-right corner, filled with Matty's presence
/// colors for online, away and Do not disturb and a hollow ring for
/// offline. The avatar adds the presence to its accessible name, as in
/// "Ana Lopez, away". Pair it with `ring_color` set to the surface under
/// the avatar, so the dot reads as cut out of it.
pub fn avatar_status(
    presence: crate::sidebar::Presence,
    palette: &Palette,
) -> crate::ui::avatar::AvatarStatus {
    use crate::sidebar::Presence;
    use crate::ui::avatar::AvatarStatus;
    let color = match presence {
        Presence::Online => palette.online,
        Presence::Away => palette.away,
        Presence::Dnd => palette.dnd,
        Presence::Offline => return AvatarStatus::Offline,
    };
    AvatarStatus::Custom {
        color,
        label: crate::account::presence_label(presence)
            .to_lowercase()
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_css_parses_in_both_modes() {
        UiTheme::from_shadcn_css(THEME_CSS, ThemeMode::Light).unwrap();
        UiTheme::from_shadcn_css(THEME_CSS, ThemeMode::Dark).unwrap();
    }

    #[test]
    fn theme_is_not_the_neutral_fallback() {
        assert_ne!(
            theme(ThemeMode::Light).colors.primary,
            UiTheme::neutral_light().colors.primary
        );
        assert_ne!(
            theme(ThemeMode::Dark).colors.sidebar,
            UiTheme::neutral_dark().colors.sidebar
        );
        assert_eq!(theme(ThemeMode::Light).radius.base, px(8.));
    }

    #[test]
    fn scaled_follows_the_spacing_unit() {
        let mut t = theme(ThemeMode::Light);
        assert_eq!(scaled(&t, metrics::HEADER_HEIGHT), px(48.));
        t.spacing.unit = px(5.);
        assert_eq!(scaled(&t, metrics::HEADER_HEIGHT), px(60.));
    }

    #[test]
    fn avatar_statuses_name_the_presence() {
        use crate::sidebar::Presence;
        let palette = Palette::for_mode(ThemeMode::Light);
        let label = |presence| avatar_status(presence, &palette).label();
        assert_eq!(label(Presence::Online), "online");
        assert_eq!(label(Presence::Away), "away");
        assert_eq!(label(Presence::Dnd), "do not disturb");
        assert_eq!(label(Presence::Offline), "offline");
        assert_eq!(
            avatar_status(Presence::Offline, &palette),
            crate::ui::avatar::AvatarStatus::Offline
        );
        assert_eq!(
            presence_name("Ana Lopez", Presence::Away),
            "Ana Lopez, away"
        );
    }
}
