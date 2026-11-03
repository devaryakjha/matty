//! Interactive S1 window for the reference machine: 50k rows in gpuicn's
//! `MessageList`. Keys: `p` prepend 200 older rows, `a` append one, `m` jump to
//! the middle, `e` jump to the end, `f` toggle frame timing (prints p50/p99
//! frame intervals every 300 frames while it runs; scroll to measure).
//!
//! `cargo run -p spike-s1 --release --features runtime_shaders`

use std::{borrow::Cow, cell::RefCell, rc::Rc, time::Instant};

use gpui_icons::LucideAssetSource;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions, div,
    px, size,
};
use gpuicn::message_list::{MessageList, MessageListState};
use gpuicn::theme::UiTheme;
use spike_s1::{INITIAL_ROWS, PAGE, Transcript, render_row};

struct Spike {
    transcript: Rc<RefCell<Transcript>>,
    list: Entity<MessageListState>,
    focus: FocusHandle,
    timing: Option<Vec<Instant>>,
}

impl Spike {
    fn key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.list.clone();
        match key {
            "p" => {
                let added = self.transcript.borrow_mut().prepend(PAGE);
                list.update(cx, |list, cx| list.prepend(added, cx));
                eprintln!(
                    "prepended {added}; {} rows",
                    self.transcript.borrow().rows.len()
                );
            }
            "a" => {
                self.transcript.borrow_mut().append();
                list.update(cx, |list, cx| list.append(1, cx));
            }
            "m" => {
                let middle = self.transcript.borrow().rows.len() / 2;
                list.update(cx, |list, cx| list.scroll_to_item(middle, cx));
            }
            "e" => list.update(cx, |list, cx| list.scroll_to_end(cx)),
            "f" => {
                self.timing = match self.timing.take() {
                    Some(_) => None,
                    None => Some(Vec::with_capacity(301)),
                };
                eprintln!(
                    "frame timing {}",
                    if self.timing.is_some() { "on" } else { "off" }
                );
                if self.timing.is_some() {
                    Self::next_frame(window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn next_frame(window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = this.update(cx, |this, cx| {
                let Some(frames) = this.timing.as_mut() else {
                    return;
                };
                frames.push(Instant::now());
                if frames.len() > 300 {
                    let mut intervals: Vec<f64> = frames
                        .windows(2)
                        .map(|pair| (pair[1] - pair[0]).as_secs_f64() * 1000.)
                        .collect();
                    intervals.sort_by(f64::total_cmp);
                    let at = |q: f64| intervals[((intervals.len() - 1) as f64 * q) as usize];
                    eprintln!(
                        "frames: p50 {:.2}ms p99 {:.2}ms max {:.2}ms",
                        at(0.5),
                        at(0.99),
                        at(1.)
                    );
                    frames.clear();
                }
                Self::next_frame(window, cx);
                window.refresh();
            });
        });
    }
}

impl Render for Spike {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = UiTheme::read(cx);
        let transcript = self.transcript.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    this.key(&event.keystroke.key, window, cx)
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background)
            .text_color(theme.colors.foreground)
            .font_family(theme.fonts.body.clone())
            .text_size(theme.text(14.))
            .child(MessageList::new("s1", &self.list, move |index, _, cx| {
                render_row(transcript.borrow().rows[index], None, cx)
            }))
    }
}

fn main() {
    gpui_kit::platform::application()
        .with_assets(LucideAssetSource)
        .run(|cx: &mut App| {
            gpuicn::theme::init(cx);
            cx.text_system()
                .add_fonts(vec![
                    Cow::Borrowed(include_bytes!(
                        "../../../crates/matty-app/assets/fonts/Geist-Regular.ttf"
                    )),
                    Cow::Borrowed(include_bytes!(
                        "../../../crates/matty-app/assets/fonts/Geist-Medium.ttf"
                    )),
                ])
                .expect("load Geist");
            let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let focus = cx.focus_handle();
                        window.focus(&focus, cx);
                        Spike {
                            transcript: Rc::new(RefCell::new(Transcript::new(INITIAL_ROWS))),
                            list: cx.new(|cx| MessageListState::new(INITIAL_ROWS, cx)),
                            focus,
                            timing: None,
                        }
                    })
                },
            )
            .expect("open the S1 window");
            cx.activate(true);
        });
}
