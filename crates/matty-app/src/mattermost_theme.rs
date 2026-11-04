//! Mattermost custom themes (#214, PLAN §5, §7, §8).
//!
//! Mattermost keeps the user's theme in the `theme` preference category:
//! the name `""` holds the theme for every team, and a team's id holds one
//! for that team alone. The value is the 20-odd-color JSON people share
//! (`sidebarBg`, `centerChannelBg`, `buttonBg`, …). [`Theme`] reads it the
//! way the web app does, [`Theme::ui`] maps it onto gpuicn's tokens and
//! Matty's [`Palette`], and [`Theme::failing`] lists the color pairs that
//! fall below WCAG AA, which Preferences warns about.
//!
//! [`ServerThemes`] is the global the window keeps current from the
//! account's preferences and the team on screen; `apply_theme` installs
//! its theme when Preferences → Appearance says to use it.

use std::collections::BTreeMap;

use gpui_kit::{App, Global, Rgba};
use matty_api::models::Preference;

use crate::contrast::{self, Pair, over};
use crate::style::{self, Palette};
use crate::ui::theme::{ThemeMode, UiTheme};

/// The preference category Mattermost keeps themes in.
pub const CATEGORY: &str = "theme";

macro_rules! theme_colors {
    ($($field:ident = $key:literal),* $(,)?) => {
        /// A Mattermost theme's colors, one per key of its JSON.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Colors {
            $(#[doc = concat!("`", $key, "`.")] pub $field: Rgba,)*
        }

        /// The JSON keys [`Colors`] reads, in its field order.
        pub const KEYS: &[&str] = &[$($key),*];

        impl Colors {
            /// Reads each color from `json`, keeping `fallback`'s where a
            /// key is missing or not a color, as the web app fills a
            /// theme's gaps from Denim.
            fn read(json: &serde_json::Map<String, serde_json::Value>, fallback: &Self) -> Self {
                let color = |key: &str| json.get(key).and_then(|v| v.as_str()).and_then(parse_color);
                Self {
                    $($field: color($key).unwrap_or(fallback.$field),)*
                }
            }
        }
    };
}

theme_colors! {
    sidebar_bg = "sidebarBg",
    sidebar_text = "sidebarText",
    sidebar_unread_text = "sidebarUnreadText",
    sidebar_text_hover_bg = "sidebarTextHoverBg",
    sidebar_text_active_border = "sidebarTextActiveBorder",
    sidebar_text_active_color = "sidebarTextActiveColor",
    sidebar_header_bg = "sidebarHeaderBg",
    sidebar_header_text_color = "sidebarHeaderTextColor",
    online_indicator = "onlineIndicator",
    away_indicator = "awayIndicator",
    dnd_indicator = "dndIndicator",
    mention_bg = "mentionBg",
    mention_color = "mentionColor",
    center_channel_bg = "centerChannelBg",
    center_channel_color = "centerChannelColor",
    new_message_separator = "newMessageSeparator",
    link_color = "linkColor",
    button_bg = "buttonBg",
    button_color = "buttonColor",
    error_text_color = "errorTextColor",
    mention_highlight_bg = "mentionHighlightBg",
    mention_highlight_link = "mentionHighlightLink",
}

/// Mattermost's built-in themes, as the web app defines them
/// (`mattermost-redux/src/constants/preferences.ts`), by `type`.
pub const BUILT_IN: [(&str, &str); 5] = [
    ("Denim", DENIM),
    ("Sapphire", SAPPHIRE),
    ("Quartz", QUARTZ),
    ("Indigo", INDIGO),
    ("Onyx", ONYX),
];

const DENIM: &str = r##"{"type":"Denim","sidebarBg":"#1e325c","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#28427b","sidebarTextActiveBorder":"#5d89ea","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#192a4d","sidebarHeaderTextColor":"#ffffff","onlineIndicator":"#3db887","awayIndicator":"#ffbc1f","dndIndicator":"#d24b4e","mentionBg":"#ffffff","mentionColor":"#1e325c","centerChannelBg":"#ffffff","centerChannelColor":"#3f4350","newMessageSeparator":"#cc8f00","linkColor":"#1c58d9","buttonBg":"#1c58d9","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#ffd470","mentionHighlightLink":"#1b1d22","codeTheme":"github"}"##;
const SAPPHIRE: &str = r##"{"type":"Sapphire","sidebarBg":"#1543a3","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#2a58ba","sidebarTextActiveBorder":"#57b5f0","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#123b91","sidebarHeaderTextColor":"#ffffff","onlineIndicator":"#3db887","awayIndicator":"#ffbc1f","dndIndicator":"#d24b4e","mentionBg":"#ffffff","mentionColor":"#174ab5","centerChannelBg":"#ffffff","centerChannelColor":"#3f4350","newMessageSeparator":"#15b7b7","linkColor":"#1c58d9","buttonBg":"#1c58d9","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#7ff0f0","mentionHighlightLink":"#0d6e6e","codeTheme":"github"}"##;
const QUARTZ: &str = r##"{"type":"Quartz","sidebarBg":"#f4f4f6","sidebarText":"#1f2228","sidebarUnreadText":"#1f2228","sidebarTextHoverBg":"#e8eaed","sidebarTextActiveBorder":"#32a4ec","sidebarTextActiveColor":"#1f2228","sidebarHeaderBg":"#ebecef","sidebarHeaderTextColor":"#1f2228","onlineIndicator":"#3db887","awayIndicator":"#f5ab07","dndIndicator":"#d24b4e","mentionBg":"#1c58d9","mentionColor":"#ffffff","centerChannelBg":"#ffffff","centerChannelColor":"#3f4350","newMessageSeparator":"#15b7b7","linkColor":"#1c58d9","buttonBg":"#1c58d9","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#7ff0f0","mentionHighlightLink":"#0d6e6e","codeTheme":"github"}"##;
const INDIGO: &str = r##"{"type":"Indigo","sidebarBg":"#151e32","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#222c3f","sidebarTextActiveBorder":"#4a7ce8","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#182339","sidebarHeaderTextColor":"#ffffff","onlineIndicator":"#3db887","awayIndicator":"#f5ab00","dndIndicator":"#d24b4e","mentionBg":"#4a7ce8","mentionColor":"#ffffff","centerChannelBg":"#111827","centerChannelColor":"#dddfe4","newMessageSeparator":"#81a3ef","linkColor":"#5d89ea","buttonBg":"#4a7ce8","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#133a91","mentionHighlightLink":"#a4f4f4","codeTheme":"solarized-dark"}"##;
const ONYX: &str = r##"{"type":"Onyx","sidebarBg":"#202228","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#25262a","sidebarTextActiveBorder":"#4a7ce8","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#24272d","sidebarHeaderTextColor":"#ffffff","onlineIndicator":"#3db887","awayIndicator":"#f5ab00","dndIndicator":"#d24b4e","mentionBg":"#4b7ce7","mentionColor":"#ffffff","centerChannelBg":"#191b1f","centerChannelColor":"#e3e4e8","newMessageSeparator":"#1adbdb","linkColor":"#5d89ea","buttonBg":"#4a7ce8","buttonColor":"#ffffff","errorTextColor":"#da6c6e","mentionHighlightBg":"#0d6e6e","mentionHighlightLink":"#a4f4f4","codeTheme":"monokai"}"##;

/// How strongly Mattermost draws secondary text over its surface: read
/// channels in the sidebar and timestamps in the channel view.
const QUIET_TEXT: f32 = 0.72;
/// Hover and muted fills, as a tint of the text color.
const TINT: f32 = 0.08;
/// Borders, as a tint of the text color.
const BORDER: f32 = 0.16;

const WHITE: Rgba = Rgba {
    r: 1.,
    g: 1.,
    b: 1.,
    a: 1.,
};
const BLACK: Rgba = Rgba {
    r: 0.,
    g: 0.,
    b: 0.,
    a: 1.,
};

/// A Mattermost theme: its name and colors.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The `type`: a built-in's name such as "Denim", or "custom".
    pub kind: String,
    /// The code block theme, such as "github" or "monokai".
    pub code_theme: String,
    pub colors: Colors,
}

impl Theme {
    /// Denim, Mattermost's default theme.
    pub fn denim() -> Self {
        Self::built_in("Denim").expect("Denim is built in")
    }

    /// A built-in theme by `type`, such as "Onyx".
    pub fn built_in(kind: &str) -> Option<Self> {
        let (_, json) = BUILT_IN.iter().find(|(name, _)| *name == kind)?;
        let map = serde_json::from_str::<serde_json::Map<_, _>>(json).ok()?;
        let base = Colors::read(&map, &BLACK_COLORS);
        Some(Self::from_map(&map, base))
    }

    /// Reads a theme preference's value as the web app does: a built-in's
    /// `type` means that built-in, whatever colors come with it; any other
    /// theme keeps its colors and takes Denim's for the ones it lacks. A
    /// value that is not a JSON object is Denim.
    pub fn parse(value: &str) -> Self {
        let Ok(mut map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(value)
        else {
            return Self::denim();
        };
        if let Some(theme) = map
            .get("type")
            .and_then(|v| v.as_str())
            .filter(|kind| *kind != "custom")
            .and_then(Self::built_in)
        {
            return theme;
        }
        // Older themes spelled `mentionBg` as `mentionBj`.
        if !map.contains_key("mentionBg")
            && let Some(old) = map.get("mentionBj").cloned()
        {
            map.insert("mentionBg".into(), old);
        }
        Self::from_map(&map, Self::denim().colors)
    }

    fn from_map(map: &serde_json::Map<String, serde_json::Value>, fallback: Colors) -> Self {
        let text = |key: &str, default: &str| {
            map.get(key)
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .unwrap_or(default)
                .to_owned()
        };
        Self {
            kind: text("type", "custom"),
            code_theme: text("codeTheme", "github"),
            colors: Colors::read(map, &fallback),
        }
    }

    /// The name Preferences shows: a built-in's, else "Custom theme".
    pub fn name(&self) -> &str {
        match BUILT_IN.iter().find(|(name, _)| *name == self.kind) {
            Some((name, _)) => name,
            None => "Custom theme",
        }
    }

    /// Light when the channel view's background is light, as text on it
    /// reads better in black than in white.
    pub fn mode(&self) -> ThemeMode {
        let bg = over(self.colors.center_channel_bg, WHITE);
        if contrast::ratio(bg, BLACK) >= contrast::ratio(bg, WHITE) {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        }
    }

    /// The theme as gpuicn tokens on Matty's theme for its mode, so radius,
    /// fonts and chart colors stay Matty's, and the colors gpuicn has no
    /// token for as Matty's [`Palette`].
    ///
    /// Translucent colors are flattened onto the surface they sit on. The
    /// channel view's tints follow Mattermost's own: hover and muted fills
    /// are its text at 8%, borders at 16%, quiet text at 72%.
    pub fn ui(&self) -> (UiTheme, Palette) {
        let m = &self.colors;
        let mode = self.mode();
        let mut theme = style::theme(mode);
        let backdrop = match mode {
            ThemeMode::Light => WHITE,
            ThemeMode::Dark => BLACK,
        };
        let bg = over(m.center_channel_bg, backdrop);
        let fg = over(m.center_channel_color, bg);
        let tint = |alpha: f32| over(fg.opacity(alpha), bg);
        let side = over(m.sidebar_bg, backdrop);
        let on_side = |color: Rgba| over(color, side);

        let c = &mut theme.colors;
        c.background = bg;
        c.foreground = fg;
        c.card = bg;
        c.card_foreground = fg;
        c.popover = bg;
        c.popover_foreground = fg;
        c.primary = over(m.button_bg, bg);
        c.primary_foreground = over(m.button_color, c.primary);
        c.secondary = tint(TINT);
        c.secondary_foreground = fg;
        c.muted = tint(TINT);
        c.muted_foreground = tint(QUIET_TEXT);
        c.accent = tint(TINT);
        c.accent_foreground = fg;
        c.destructive = over(m.error_text_color, bg);
        c.destructive_foreground = readable_on(c.destructive);
        c.success = over(m.online_indicator, bg);
        c.success_foreground = readable_on(c.success);
        c.warning = over(m.away_indicator, bg);
        c.warning_foreground = readable_on(c.warning);
        c.info = over(m.link_color, bg);
        c.info_foreground = readable_on(c.info);
        c.border = tint(BORDER);
        c.input = tint(BORDER);
        c.ring = c.primary;
        c.sidebar = side;
        c.sidebar_foreground = on_side(m.sidebar_unread_text);
        c.sidebar_primary = on_side(m.sidebar_text_active_border);
        c.sidebar_primary_foreground = readable_on(c.sidebar_primary);
        c.sidebar_accent = on_side(m.sidebar_text_hover_bg);
        c.sidebar_accent_foreground = over(m.sidebar_text_active_color, c.sidebar_accent);
        c.sidebar_border = on_side(m.sidebar_text.opacity(BORDER));
        c.sidebar_ring = c.sidebar_primary;

        let mention = on_side(m.mention_bg);
        let palette = Palette {
            online: m.online_indicator,
            away: m.away_indicator,
            dnd: m.dnd_indicator,
            sidebar_muted: on_side(m.sidebar_text.opacity(QUIET_TEXT)),
            mention,
            mention_foreground: over(m.mention_color, mention),
        };
        (theme, palette)
    }

    /// The pairs that fall below WCAG AA once mapped: 4.5:1 for text,
    /// 3:1 for other UI.
    pub fn failing(&self) -> Vec<Pair> {
        let (theme, palette) = self.ui();
        contrast::pairs_of(&theme.colors, &palette)
            .into_iter()
            .filter(|pair| !pair.passes())
            .collect()
    }
}

const BLACK_COLORS: Colors = Colors {
    sidebar_bg: BLACK,
    sidebar_text: BLACK,
    sidebar_unread_text: BLACK,
    sidebar_text_hover_bg: BLACK,
    sidebar_text_active_border: BLACK,
    sidebar_text_active_color: BLACK,
    sidebar_header_bg: BLACK,
    sidebar_header_text_color: BLACK,
    online_indicator: BLACK,
    away_indicator: BLACK,
    dnd_indicator: BLACK,
    mention_bg: BLACK,
    mention_color: BLACK,
    center_channel_bg: BLACK,
    center_channel_color: BLACK,
    new_message_separator: BLACK,
    link_color: BLACK,
    button_bg: BLACK,
    button_color: BLACK,
    error_text_color: BLACK,
    mention_highlight_bg: BLACK,
    mention_highlight_link: BLACK,
};

/// White or black, whichever reads better on `bg`.
fn readable_on(bg: Rgba) -> Rgba {
    if contrast::ratio(WHITE, bg) >= contrast::ratio(BLACK, bg) {
        WHITE
    } else {
        BLACK
    }
}

/// A CSS color as Mattermost themes write them: `#rgb`, `#rrggbb`,
/// `#rrggbbaa`, `rgb(r, g, b)` or `rgba(r, g, b, a)`.
pub fn parse_color(value: &str) -> Option<Rgba> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()?;
        let pair = |i: usize| f32::from(digits[i] * 16 + digits[i + 1]) / 255.;
        let single = |i: usize| f32::from(digits[i] * 17) / 255.;
        return match digits.len() {
            3 => Some(Rgba {
                r: single(0),
                g: single(1),
                b: single(2),
                a: 1.,
            }),
            6 | 8 => Some(Rgba {
                r: pair(0),
                g: pair(2),
                b: pair(4),
                a: if digits.len() == 8 { pair(6) } else { 1. },
            }),
            _ => None,
        };
    }
    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f32> = inner
        .split(',')
        .map(|part| part.trim().parse::<f32>().ok())
        .collect::<Option<_>>()?;
    let channel = |v: f32| (v / 255.).clamp(0., 1.);
    match parts.as_slice() {
        [r, g, b] => Some(Rgba {
            r: channel(*r),
            g: channel(*g),
            b: channel(*b),
            a: 1.,
        }),
        [r, g, b, a] => Some(Rgba {
            r: channel(*r),
            g: channel(*g),
            b: channel(*b),
            a: a.clamp(0., 1.),
        }),
        _ => None,
    }
}

/// The account's themes and the one for the team on screen.
#[derive(Clone, Debug, Default)]
pub struct ServerThemes {
    /// Each `theme` preference's value by name: `""` for every team, else
    /// a team id. `None` until the account's preferences arrive.
    values: Option<BTreeMap<String, String>>,
    team: Option<String>,
    shown: Option<Theme>,
}

impl Global for ServerThemes {}

impl ServerThemes {
    /// The theme for the team on screen, once the account's preferences
    /// have arrived.
    pub fn current(cx: &App) -> Option<&Theme> {
        cx.try_global::<Self>()?.shown.as_ref()
    }

    /// Takes the account's `theme` preferences (when `preferences` is
    /// given; it holds every account preference, so a theme missing from
    /// it was deleted) and the team on screen. Returns whether the theme
    /// to show changed.
    pub fn update(cx: &mut App, preferences: Option<&[Preference]>, team: Option<&str>) -> bool {
        let mut state = cx.try_global::<Self>().cloned().unwrap_or_default();
        if let Some(preferences) = preferences {
            state.values = Some(
                preferences
                    .iter()
                    .filter(|p| p.category == CATEGORY)
                    .map(|p| (p.name.clone(), p.value.clone()))
                    .collect(),
            );
        }
        state.team = team.map(str::to_owned);
        let shown = state
            .values
            .as_ref()
            .map(|values| resolve(values, state.team.as_deref()));
        let changed = shown != state.shown;
        state.shown = shown;
        cx.set_global(state);
        changed
    }

    /// Forgets the account's themes, as on signing out. Returns whether a
    /// theme was showing.
    pub fn clear(cx: &mut App) -> bool {
        let had = Self::current(cx).is_some();
        cx.set_global(Self::default());
        had
    }

    /// Shows `theme` with no account behind it, for `MATTY_PREVIEW`.
    pub fn preview(theme: Theme, cx: &mut App) {
        cx.set_global(Self {
            values: Some(BTreeMap::new()),
            team: None,
            shown: Some(theme),
        });
    }
}

/// The team's own theme, else the one for every team, else Denim.
fn resolve(values: &BTreeMap<String, String>, team: Option<&str>) -> Theme {
    team.and_then(|team| values.get(team))
        .or_else(|| values.get(""))
        .map_or_else(Theme::denim, |value| Theme::parse(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rgb;

    fn hex(color: Rgba) -> u32 {
        let byte = |c: f32| (c * 255.).round() as u32;
        (byte(color.r) << 16) | (byte(color.g) << 8) | byte(color.b)
    }

    #[test]
    fn built_in_themes_are_complete() {
        for (name, json) in BUILT_IN {
            let map: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(json).unwrap();
            for key in KEYS {
                let value = map.get(*key).and_then(|v| v.as_str());
                assert!(
                    value.and_then(parse_color).is_some(),
                    "{name} has no color for {key}"
                );
            }
            assert_eq!(Theme::built_in(name).unwrap().name(), name);
        }
    }

    #[test]
    fn parses_mattermost_colors() {
        assert_eq!(parse_color("#1e325c"), Some(rgb(0x1e325c)));
        assert_eq!(parse_color(" #FFF "), Some(rgb(0xffffff)));
        assert_eq!(parse_color("rgb(30, 50, 92)"), Some(rgb(0x1e325c)));
        let translucent = parse_color("rgba(255,255,255,0.5)").unwrap();
        assert!((translucent.a - 0.5).abs() < 1e-6);
        assert!((parse_color("#00000080").unwrap().a - 128. / 255.).abs() < 1e-6);
        assert_eq!(parse_color("blue"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#gggggg"), None);
    }

    #[test]
    fn denim_maps_onto_light_tokens() {
        let theme = Theme::denim();
        assert_eq!(theme.mode(), ThemeMode::Light);
        let (ui, palette) = theme.ui();
        let c = ui.colors;
        assert_eq!(ui.mode, ThemeMode::Light);
        assert_eq!(hex(c.background), 0xffffff);
        assert_eq!(hex(c.foreground), 0x3f4350);
        assert_eq!(hex(c.primary), 0x1c58d9);
        assert_eq!(hex(c.primary_foreground), 0xffffff);
        assert_eq!(hex(c.destructive), 0xd24b4e);
        assert_eq!(hex(c.sidebar), 0x1e325c);
        assert_eq!(hex(c.sidebar_foreground), 0xffffff);
        assert_eq!(hex(c.sidebar_accent), 0x28427b);
        assert_eq!(hex(c.sidebar_ring), 0x5d89ea);
        assert_eq!(hex(palette.online), 0x3db887);
        assert_eq!(hex(palette.away), 0xffbc1f);
        assert_eq!(hex(palette.dnd), 0xd24b4e);
        assert_eq!(hex(palette.mention), 0xffffff);
        assert_eq!(hex(palette.mention_foreground), 0x1e325c);
        // Read channels: white at 72% over the navy sidebar.
        assert_eq!(hex(palette.sidebar_muted), 0xc0c6d1);
        // Matty's radius and fonts stay.
        assert_eq!(ui.radius, style::theme(ThemeMode::Light).radius);
    }

    #[test]
    fn sapphire_and_quartz_map_onto_light_tokens() {
        let (ui, palette) = Theme::built_in("Sapphire").unwrap().ui();
        assert_eq!(ui.mode, ThemeMode::Light);
        assert_eq!(hex(ui.colors.sidebar), 0x1543a3);
        assert_eq!(hex(ui.colors.sidebar_ring), 0x57b5f0);
        assert_eq!(hex(palette.mention_foreground), 0x174ab5);

        let (ui, palette) = Theme::built_in("Quartz").unwrap().ui();
        assert_eq!(ui.mode, ThemeMode::Light);
        assert_eq!(hex(ui.colors.sidebar), 0xf4f4f6);
        assert_eq!(hex(ui.colors.sidebar_foreground), 0x1f2228);
        assert_eq!(hex(palette.mention), 0x1c58d9);
        assert_eq!(hex(palette.mention_foreground), 0xffffff);
        assert_eq!(hex(palette.away), 0xf5ab07);
    }

    #[test]
    fn indigo_and_onyx_map_onto_dark_tokens() {
        let (ui, palette) = Theme::built_in("Indigo").unwrap().ui();
        assert_eq!(ui.mode, ThemeMode::Dark);
        assert_eq!(hex(ui.colors.background), 0x111827);
        assert_eq!(hex(ui.colors.foreground), 0xdddfe4);
        assert_eq!(hex(ui.colors.sidebar), 0x151e32);
        assert_eq!(hex(ui.colors.primary), 0x4a7ce8);
        assert_eq!(hex(palette.mention), 0x4a7ce8);

        let (ui, _) = Theme::built_in("Onyx").unwrap().ui();
        assert_eq!(ui.mode, ThemeMode::Dark);
        assert_eq!(hex(ui.colors.background), 0x191b1f);
        assert_eq!(hex(ui.colors.foreground), 0xe3e4e8);
        assert_eq!(hex(ui.colors.destructive), 0xda6c6e);
        assert_eq!(ui.radius, style::theme(ThemeMode::Dark).radius);
    }

    #[test]
    fn a_built_in_type_means_the_built_in() {
        let theme = Theme::parse(r##"{"type":"Onyx","sidebarBg":"#ff0000"}"##);
        assert_eq!(theme, Theme::built_in("Onyx").unwrap());
    }

    #[test]
    fn a_custom_theme_fills_its_gaps_from_denim() {
        let theme = Theme::parse(
            r##"{"type":"custom","sidebarBg":"#6a1b9a","centerChannelBg":"#FAFAFA","mentionBj":"#ffeb3b","codeTheme":"monokai"}"##,
        );
        assert_eq!(theme.name(), "Custom theme");
        assert_eq!(theme.code_theme, "monokai");
        assert_eq!(theme.colors.sidebar_bg, rgb(0x6a1b9a));
        assert_eq!(theme.colors.center_channel_bg, rgb(0xfafafa));
        assert_eq!(theme.colors.mention_bg, rgb(0xffeb3b));
        assert_eq!(theme.colors.button_bg, Theme::denim().colors.button_bg);
        assert_eq!(Theme::parse("not json"), Theme::denim());
    }

    #[test]
    fn the_teams_theme_wins_over_the_default() {
        let values: BTreeMap<String, String> = [
            ("".to_owned(), ONYX.to_owned()),
            ("t2".to_owned(), QUARTZ.to_owned()),
        ]
        .into();
        assert_eq!(resolve(&values, Some("t1")).kind, "Onyx");
        assert_eq!(resolve(&values, Some("t2")).kind, "Quartz");
        assert_eq!(resolve(&values, None).kind, "Onyx");
        assert_eq!(resolve(&BTreeMap::new(), Some("t1")).kind, "Denim");
    }

    #[test]
    fn contrast_check_flags_failing_pairs() {
        // Mattermost's own green online dot is too faint on white.
        let names: Vec<&str> = Theme::denim().failing().iter().map(|p| p.name).collect();
        assert!(names.contains(&"Online dot on messages"), "{names:?}");
        assert!(!names.contains(&"Message text"), "{names:?}");
        // White text on a pale yellow sidebar fails as text.
        let pale = Theme::parse(
            r##"{"type":"custom","sidebarBg":"#fff3a0","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff"}"##,
        );
        let failing = pale.failing();
        let sidebar = failing
            .iter()
            .find(|p| p.name == "Sidebar text")
            .expect("sidebar text fails");
        assert!(sidebar.ratio() < 4.5);
        // A well-chosen theme passes every pair.
        let calm = Theme::parse(
            r##"{"type":"custom","sidebarBg":"#1b1f2a","sidebarText":"#e8eaef","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#2a2f3c","sidebarTextActiveBorder":"#8aa6ff","sidebarTextActiveColor":"#ffffff","onlineIndicator":"#16a34a","awayIndicator":"#b86e00","dndIndicator":"#d42020","mentionBg":"#ffffff","mentionColor":"#1b1f2a","centerChannelBg":"#ffffff","centerChannelColor":"#1b1f2a","linkColor":"#3056d3","buttonBg":"#3056d3","buttonColor":"#ffffff","errorTextColor":"#c01818"}"##,
        );
        let failing: Vec<String> = calm
            .failing()
            .iter()
            .map(|p| format!("{} {:.2}", p.name, p.ratio()))
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
    }
}
