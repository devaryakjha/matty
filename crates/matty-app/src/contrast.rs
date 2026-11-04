//! WCAG 2.1 contrast checks for every foreground/background pair Matty
//! draws (#172). The table in `docs/accessibility/contrast.md` is written
//! from [`pairs`]; a test fails when a pair drops below its threshold or
//! when the table is out of date.

use gpui_kit::Rgba;

use crate::style::{Palette, theme};
use crate::ui::theme::ThemeMode;

/// What a pair is: text needs 4.5:1, other UI (focus rings, presence dots,
/// icons that carry meaning) needs 3:1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Ui,
}

impl Kind {
    /// The WCAG AA minimum for this kind.
    pub fn minimum(self) -> f32 {
        match self {
            Kind::Text => 4.5,
            Kind::Ui => 3.0,
        }
    }
}

/// One foreground drawn on one background.
#[derive(Clone, Debug)]
pub struct Pair {
    pub name: &'static str,
    pub kind: Kind,
    pub fg: Rgba,
    pub bg: Rgba,
}

impl Pair {
    pub fn ratio(&self) -> f32 {
        ratio(self.fg, self.bg)
    }

    pub fn passes(&self) -> bool {
        self.ratio() >= self.kind.minimum()
    }
}

/// `top` composited over the opaque `bottom`.
pub fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let mix = |t: f32, b: f32| t * top.a + b * (1. - top.a);
    Rgba {
        r: mix(top.r, bottom.r),
        g: mix(top.g, bottom.g),
        b: mix(top.b, bottom.b),
        a: 1.,
    }
}

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

/// The WCAG 2.1 contrast ratio of two opaque colors.
pub fn ratio(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a) + 0.05, luminance(b) + 0.05);
    la.max(lb) / la.min(lb)
}

/// Every pair the app draws in `mode`. Translucent fills (hover, mention
/// and error tints) are composited over the surface they sit on.
pub fn pairs(mode: ThemeMode) -> Vec<Pair> {
    let c = theme(mode).colors;
    let p = Palette::for_mode(mode);
    // gpuicn's Message: mention rows tint primary at 6%, hover is muted at
    // 50%. Matty's offline banner tints destructive at 8%.
    let mention = over(c.primary.opacity(0.06), c.background);
    let hover = over(c.muted.opacity(0.5), c.background);
    let error = over(c.destructive.opacity(0.08), c.background);
    let chip = over(c.primary.opacity(0.1), c.background);
    let text = |name, fg, bg| Pair {
        name,
        kind: Kind::Text,
        fg,
        bg,
    };
    let ui = |name, fg, bg| Pair {
        name,
        kind: Kind::Ui,
        fg,
        bg,
    };
    vec![
        text("Message text", c.foreground, c.background),
        text(
            "Timestamps and muted text",
            c.muted_foreground,
            c.background,
        ),
        text("Message text on a mention row", c.foreground, mention),
        text("Timestamps on a mention row", c.muted_foreground, mention),
        text("Timestamps on a hovered row", c.muted_foreground, hover),
        text("Mention chip", c.primary, chip),
        text("Links and primary text", c.primary, c.background),
        text("Errors", c.destructive, c.background),
        text("Error banner", c.destructive, error),
        text("Sidebar text", c.sidebar_foreground, c.sidebar),
        text("Sidebar muted text", c.muted_foreground, c.sidebar),
        text(
            "Selected sidebar row",
            c.sidebar_accent_foreground,
            c.sidebar_accent,
        ),
        text("Hovered list row", c.foreground, c.accent),
        text(
            "Muted text on a hovered list row",
            c.muted_foreground,
            c.accent,
        ),
        text("Muted surfaces", c.muted_foreground, c.muted),
        text("Cards", c.card_foreground, c.card),
        text("Muted text on cards", c.muted_foreground, c.card),
        text("Menus and popovers", c.popover_foreground, c.popover),
        text("Muted text in popovers", c.muted_foreground, c.popover),
        text(
            "Primary buttons and badges",
            c.primary_foreground,
            c.primary,
        ),
        ui("Focus ring on the window", c.ring, c.background),
        ui("Focus ring in the sidebar", c.ring, c.sidebar),
        ui("Focus ring in popovers", c.ring, c.popover),
        ui("Unread badge on the sidebar", c.primary, c.sidebar),
        ui("Online dot", p.online, c.sidebar),
        ui("Away dot", p.away, c.sidebar),
        ui("Do not disturb dot", p.dnd, c.sidebar),
        ui("Offline ring", c.muted_foreground, c.sidebar),
        ui("Online dot on messages", p.online, c.background),
        ui("Away dot on messages", p.away, c.background),
        ui("Do not disturb dot on messages", p.dnd, c.background),
    ]
}

/// The Markdown table for `docs/accessibility/contrast.md`.
pub fn table() -> String {
    let mut out = String::from(
        "# Contrast\n\n\
         Every foreground/background pair Matty draws, with its WCAG 2.1 \
         contrast ratio. Text needs 4.5:1 and other UI (focus rings, \
         presence dots, badges) needs 3:1. Translucent fills are composited \
         over the surface they sit on.\n\n\
         This file is generated by `crates/matty-app/src/contrast.rs`. To \
         refresh it, run\n\
         `MATTY_WRITE_CONTRAST=1 cargo test -p matty-app contrast`.\n",
    );
    for (mode, title) in [(ThemeMode::Light, "Light"), (ThemeMode::Dark, "Dark")] {
        out.push_str(&format!(
            "\n## {title}\n\n| Pair | Kind | Ratio | Minimum |\n|---|---|---|---|\n"
        ));
        for pair in pairs(mode) {
            let kind = match pair.kind {
                Kind::Text => "Text",
                Kind::Ui => "UI",
            };
            out.push_str(&format!(
                "| {} | {kind} | {:.2}:1 | {}:1 |\n",
                pair.name,
                pair.ratio(),
                pair.kind.minimum()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/accessibility/contrast.md"
    );

    #[test]
    fn contrast_pairs_meet_wcag_aa() {
        let failing: Vec<String> = [ThemeMode::Light, ThemeMode::Dark]
            .into_iter()
            .flat_map(|mode| {
                pairs(mode)
                    .into_iter()
                    .filter(|pair| !pair.passes())
                    .map(move |pair| {
                        format!(
                            "{mode:?} {}: {:.2} < {}",
                            pair.name,
                            pair.ratio(),
                            pair.kind.minimum()
                        )
                    })
            })
            .collect();
        assert!(failing.is_empty(), "{}", failing.join("\n"));
    }

    #[test]
    fn contrast_table_is_current() {
        let table = table();
        if std::env::var_os("MATTY_WRITE_CONTRAST").is_some() {
            std::fs::write(DOC, &table).unwrap();
        }
        let doc = std::fs::read_to_string(DOC).unwrap_or_default();
        assert!(
            doc == table,
            "docs/accessibility/contrast.md is out of date; run \
             MATTY_WRITE_CONTRAST=1 cargo test -p matty-app contrast"
        );
    }

    #[test]
    fn over_composites_alpha() {
        let white = Rgba {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 1.,
        };
        let black = Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.5,
        };
        let grey = over(black, white);
        assert!((grey.r - 0.5).abs() < 1e-6 && grey.a == 1.);
        assert!((ratio(white, white) - 1.).abs() < 1e-6);
    }
}
