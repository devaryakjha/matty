//! The formatting toolbar (#219): strikethrough, link, quote, lists and
//! headings in a row above the composer, shown with the toolbar's
//! "Formatting" toggle and kept as a setting. Every button runs the same
//! code as its shortcut, so a second press takes the format off again.

use std::ops::Range;

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
};

use super::{ChannelView, unwrap_selection};
use crate::settings::SettingsStore;
use crate::style::{self, metrics, scaled};
use crate::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::ui::composer::ComposerState;
use crate::ui::theme::UiTheme;

/// Wraps the composer's selection in `before` and `after`, or takes them
/// off when it is already wrapped (#115). The format shortcuts and the
/// toolbar buttons share it.
pub(crate) fn toggle_format(
    composer: &mut ComposerState,
    before: &str,
    after: &str,
    window: &mut Window,
    cx: &mut Context<ComposerState>,
) {
    let value = composer.value(cx);
    if let Some((wrapped, selection)) =
        unwrap_selection(&value, composer.selection(cx), before, after)
    {
        let inside = value[wrapped.start + before.len()..wrapped.end - after.len()].to_string();
        composer.input().update(cx, |input, cx| {
            input.set_selected_range(wrapped, cx);
            input.replace(inside, window, cx);
            input.set_selected_range(selection, cx);
        });
        return;
    }
    composer.wrap_selection(before, after, window, cx);
    if after == "](url)" {
        // The link's `url` is selected, ready to paste over.
        let end = composer.selection(cx).end;
        composer.set_selection(end + 2..end + 5, cx);
    }
}

/// A block format that starts each line: a quote, a list or a heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineFormat {
    Quote,
    Bullets,
    Numbers,
    Heading,
}

impl LineFormat {
    /// The marker the `index`th selected line starts with.
    fn marker(self, index: usize) -> String {
        match self {
            Self::Quote => "> ".into(),
            Self::Bullets => "- ".into(),
            Self::Numbers => format!("{}. ", index + 1),
            Self::Heading => "### ".into(),
        }
    }

    /// The marker `line` already starts with, if it has this format's.
    fn existing(self, line: &str) -> Option<usize> {
        match self {
            Self::Quote => line.starts_with("> ").then_some(2),
            Self::Bullets => line.starts_with("- ").then_some(2),
            Self::Heading => line.starts_with("### ").then_some(4),
            Self::Numbers => {
                let digits = line.chars().take_while(char::is_ascii_digit).count();
                (digits > 0 && line[digits..].starts_with(". ")).then_some(digits + 2)
            }
        }
    }
}

/// The lines `selection` touches in `value`, each marked with `format`, or
/// unmarked when every one of them already is: the range of whole lines to
/// replace, its new text, and the selection after, over the same lines.
pub(crate) fn format_lines(
    value: &str,
    selection: Range<usize>,
    format: LineFormat,
) -> (Range<usize>, String, Range<usize>) {
    let start = value[..selection.start.min(value.len())]
        .rfind('\n')
        .map_or(0, |at| at + 1);
    let end = value[selection.end.min(value.len())..]
        .find('\n')
        .map_or(value.len(), |at| selection.end + at);
    let lines: Vec<&str> = value[start..end].split('\n').collect();
    let all_marked = lines.iter().all(|line| format.existing(line).is_some());
    let text = lines
        .iter()
        .enumerate()
        .map(|(index, line)| match (all_marked, format.existing(line)) {
            (true, Some(marker)) => line[marker..].to_string(),
            // Lines already marked are not marked twice.
            (false, Some(_)) => (*line).to_string(),
            (_, None) => format!("{}{line}", format.marker(index)),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let after = start..start + text.len();
    (start..end, text, after)
}

/// Marks or unmarks the lines the composer's selection touches.
pub(crate) fn toggle_lines(
    composer: &mut ComposerState,
    format: LineFormat,
    window: &mut Window,
    cx: &mut Context<ComposerState>,
) {
    let value = composer.value(cx);
    let (range, text, selection) = format_lines(&value, composer.selection(cx), format);
    composer.input().update(cx, |input, cx| {
        input.set_selected_range(range, cx);
        input.replace(text, window, cx);
        input.set_selected_range(selection.end..selection.end, cx);
    });
}

impl ChannelView {
    /// A toolbar button: an inline format or a line format on `composer`.
    fn format_button<F>(
        id: &'static str,
        label: &'static str,
        glyph: LucideIcon,
        composer: &Entity<ComposerState>,
        run: F,
        theme: &UiTheme,
    ) -> impl IntoElement + use<F>
    where
        F: Fn(&mut ComposerState, &mut Window, &mut Context<ComposerState>) + 'static,
    {
        let composer = composer.clone();
        style::tipped(
            SharedString::from(format!("{id}.tip")),
            label,
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .aria_label(label)
                .child(
                    lucide(glyph)
                        .size(scaled(theme, metrics::ICON))
                        .text_color(theme.colors.muted_foreground),
                )
                .on_click(move |_, window, cx| {
                    composer.update(cx, |composer, cx| {
                        run(composer, window, cx);
                        composer.focus(window, cx);
                    })
                }),
        )
    }

    /// The row of formats beyond bold, italic and code, when the user
    /// turned it on.
    pub(super) fn format_bar(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        if !SettingsStore::global(cx).read(cx).get().format_bar {
            return None;
        }
        let theme = UiTheme::read(cx).clone();
        let composer = &self.composer;
        let inline = |before: &'static str, after: &'static str| {
            move |composer: &mut ComposerState, window: &mut Window, cx: &mut Context<_>| {
                toggle_format(composer, before, after, window, cx)
            }
        };
        let lines = |format: LineFormat| {
            move |composer: &mut ComposerState, window: &mut Window, cx: &mut Context<_>| {
                toggle_lines(composer, format, window, cx)
            }
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(theme.space(0.5))
                .pb(theme.space(1.))
                .child(Self::format_button(
                    "composer.format.strike",
                    "Strikethrough",
                    LucideIcon::Strikethrough,
                    composer,
                    inline("~~", "~~"),
                    &theme,
                ))
                .child(Self::format_button(
                    "composer.format.link",
                    "Link",
                    LucideIcon::Link,
                    composer,
                    inline("[", "](url)"),
                    &theme,
                ))
                .child(Self::format_button(
                    "composer.format.heading",
                    "Heading",
                    LucideIcon::Heading,
                    composer,
                    lines(LineFormat::Heading),
                    &theme,
                ))
                .child(Self::format_button(
                    "composer.format.quote",
                    "Quote",
                    LucideIcon::TextQuote,
                    composer,
                    lines(LineFormat::Quote),
                    &theme,
                ))
                .child(Self::format_button(
                    "composer.format.bullets",
                    "Bulleted list",
                    LucideIcon::List,
                    composer,
                    lines(LineFormat::Bullets),
                    &theme,
                ))
                .child(Self::format_button(
                    "composer.format.numbers",
                    "Numbered list",
                    LucideIcon::ListOrdered,
                    composer,
                    lines(LineFormat::Numbers),
                    &theme,
                )),
        )
    }
}

/// Shows or hides the formatting row, for every composer.
pub(crate) fn toggle_format_bar(cx: &mut App) {
    SettingsStore::global(cx).update(cx, |store, cx| {
        store.update(cx, |settings| settings.format_bar = !settings.format_bar)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(value: &str, selection: Range<usize>, format: LineFormat) -> String {
        let (range, text, _) = format_lines(value, selection, format);
        format!("{}{text}{}", &value[..range.start], &value[range.end..])
    }

    #[test]
    fn line_formats_mark_every_selected_line_and_toggle_off() {
        let value = "intro\nfirst\nsecond\noutro";
        // The caret on one line marks only that line.
        assert_eq!(
            apply(value, 7..7, LineFormat::Quote),
            "intro\n> first\nsecond\noutro"
        );
        // A selection across lines marks each of them; numbers count up.
        assert_eq!(
            apply(value, 8..15, LineFormat::Numbers),
            "intro\n1. first\n2. second\noutro"
        );
        assert_eq!(
            apply(value, 8..15, LineFormat::Bullets),
            "intro\n- first\n- second\noutro"
        );
        // Marked already: the same button takes the marks off.
        assert_eq!(apply("- a\n- b", 0..7, LineFormat::Bullets), "a\nb");
        assert_eq!(apply("12. a", 0..0, LineFormat::Numbers), "a");
        // Only some marked: the rest get the mark, none get it twice.
        assert_eq!(apply("### a\nb", 0..7, LineFormat::Heading), "### a\n### b");
        // An empty composer gets a marker to type after.
        assert_eq!(apply("", 0..0, LineFormat::Quote), "> ");
    }

    #[test]
    fn the_selection_after_covers_the_new_lines() {
        let (range, text, after) = format_lines("a\nb", 2..3, LineFormat::Quote);
        assert_eq!((range, text.as_str(), after), (2..3, "> b", 2..5));
    }
}
