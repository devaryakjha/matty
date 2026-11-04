//! Matty's design tokens on top of gpuicn's [`UiTheme`] (docs/DESIGN.md).
//!
//! The color theme is `assets/theme.css` in shadcn's variable format, so it
//! can be tuned in tweakcn and pasted back. This module turns it into a
//! [`UiTheme`] per mode, adds the semantic colors shadcn has no name for
//! ([`Palette`]), and names the type scale and layout metrics every view
//! shares, so a size appears once here and nowhere else.

use gpui_kit::{
    App, ElementId, FontWeight, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Rgba, SharedString, StatefulInteractiveElement as _, Styled, div, px, rgb,
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
    match UiTheme::from_shadcn_css(THEME_CSS, mode) {
        Ok(theme) => theme,
        Err(error) => {
            eprintln!("matty: theme.css did not parse ({error:?}); using the Neutral palette");
            match mode {
                ThemeMode::Light => UiTheme::neutral_light(),
                ThemeMode::Dark => UiTheme::neutral_dark(),
            }
        }
    }
}

/// Colors with a meaning shadcn's tokens do not cover (DESIGN.md §2):
/// presence. Mentions, the "New messages" line and links use gpuicn's own
/// treatment of `primary` and `destructive`, so they need no token here.
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
        match mode {
            ThemeMode::Light => Self {
                online: rgb(0x16a34a),
                away: rgb(0xcc7a00),
                dnd: rgb(0xd42020),
            },
            ThemeMode::Dark => Self {
                online: rgb(0x3fb950),
                away: rgb(0xf2b038),
                dnd: rgb(0xf0564f),
            },
        }
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
    let mut next = theme(mode);
    if cx.has_global::<UiTheme>() {
        let current = UiTheme::read(cx);
        next.text_scale = current.text_scale;
        next.spacing = current.spacing;
        next.motion = current.motion;
        if *current == next && cx.has_global::<Palette>() {
            return;
        }
    }
    cx.set_global(Palette::for_mode(mode));
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
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .tooltip(move |_, cx| text_tooltip(tip.clone(), cx))
        .child(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Rgba) -> f32 {
        fn channel(c: f32) -> f32 {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    /// WCAG 2.1 contrast ratio.
    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (la, lb) = (luminance(a) + 0.05, luminance(b) + 0.05);
        la.max(lb) / la.min(lb)
    }

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
    fn text_meets_wcag_aa_on_its_surfaces() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let c = theme(mode).colors;
            let pairs = [
                ("foreground/background", c.foreground, c.background),
                ("muted/background", c.muted_foreground, c.background),
                ("muted/sidebar", c.muted_foreground, c.sidebar),
                ("sidebar fg/sidebar", c.sidebar_foreground, c.sidebar),
                (
                    "sidebar fg/sidebar accent",
                    c.sidebar_accent_foreground,
                    c.sidebar_accent,
                ),
                ("primary fg/primary", c.primary_foreground, c.primary),
                ("popover fg/popover", c.popover_foreground, c.popover),
            ];
            for (name, fg, bg) in pairs {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{mode:?} {name}: contrast {ratio:.2} < 4.5");
            }
        }
    }

    #[test]
    fn scaled_follows_the_spacing_unit() {
        let mut t = theme(ThemeMode::Light);
        assert_eq!(scaled(&t, metrics::HEADER_HEIGHT), px(48.));
        t.spacing.unit = px(5.);
        assert_eq!(scaled(&t, metrics::HEADER_HEIGHT), px(60.));
    }
}
