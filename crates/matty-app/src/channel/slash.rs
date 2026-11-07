//! The channel composer's slash commands (#210); see [`crate::slash`].

use gpui_kit::{Context, SharedString, Window};

use super::{ChannelEvent, ChannelView};
use crate::actions::{OpenPreferences, ShowShortcuts};
use crate::channel_settings::ChannelField;
use crate::slash::{Change, Line, Local, Place, Request};

impl ChannelView {
    fn channel_id(&self) -> Option<SharedString> {
        self.model.as_ref().map(|model| model.id.clone())
    }

    /// The composer shows `query` after `/`: the commands that match now,
    /// and the server is asked for its own.
    pub(super) fn slash_typing(&mut self, query: &str, cx: &mut Context<Self>) {
        let asks = self.slash.typing(query, !self.offline);
        if let Some(channel_id) = self.channel_id() {
            for user_input in asks {
                cx.emit(ChannelEvent::Slash(Request::Suggest {
                    channel_id: channel_id.clone(),
                    root_id: SharedString::default(),
                    user_input,
                }));
            }
        }
        self.show_commands(query, cx);
    }

    /// The composer's text changed, typed or set: past a command's name,
    /// the server is asked about the whole line and its suggestions show
    /// over it; otherwise they close.
    pub(super) fn slash_line(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.slash.text_changed(text) {
            return;
        }
        if let Some(user_input) = self.slash.line_typing(text, !self.offline)
            && let Some(channel_id) = self.channel_id()
        {
            cx.emit(ChannelEvent::Slash(Request::Suggest {
                channel_id,
                root_id: SharedString::default(),
                user_input,
            }));
        }
        crate::slash::show_line(&self.slash, &self.composer, cx);
    }

    fn show_commands(&self, query: &str, cx: &mut Context<Self>) {
        let rows = self.slash.suggestions(query);
        self.composer
            .update(cx, |composer, cx| composer.set_suggestions(rows, cx));
    }

    /// Runs `text` when it is a slash command; `true` when it was, and the
    /// message is not sent.
    pub(super) fn run_slash(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match crate::slash::parse(text, Place::Channel) {
            Line::Message => false,
            Line::Local(local) => {
                self.run_local(local, window, cx);
                true
            }
            Line::Server(command) => {
                // One at a time: Enter again while it runs does nothing.
                if self.slash.running().is_some() || self.refused_offline("run command", cx) {
                    return true;
                }
                let Some(channel_id) = self.channel_id() else {
                    return true;
                };
                self.slash.started(&command);
                cx.emit(ChannelEvent::Slash(Request::Execute {
                    channel_id,
                    root_id: SharedString::default(),
                    command,
                }));
                cx.notify();
                true
            }
        }
    }

    /// The composer empties, as after sending.
    fn clear_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._draft_task = None;
        if let Some(cleared) = self.draft.sent(cx) {
            cx.emit(ChannelEvent::SaveDraft(cleared));
        }
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.draft_len = 0;
        cx.notify();
    }

    fn run_local(&mut self, local: Local, window: &mut Window, cx: &mut Context<Self>) {
        if let Local::Code(block) = local {
            // A message like any other, so it waits in the outbox offline.
            return self.send(block.into(), window, cx);
        }
        if let Local::Mute = local
            && self.refused_offline("change notifications", cx)
        {
            return;
        }
        self.clear_command(window, cx);
        match local {
            Local::Shortcuts => window.dispatch_action(Box::new(ShowShortcuts), cx),
            Local::Settings => window.dispatch_action(Box::new(OpenPreferences), cx),
            Local::Leave => self.leave.open(window, cx),
            Local::Header => self.edit_channel(ChannelField::Header, window, cx),
            Local::Purpose => self.edit_channel(ChannelField::Purpose, window, cx),
            Local::Mute => {
                if let Some(model) = self.model.as_mut().map(std::rc::Rc::make_mut) {
                    model.muted = !model.muted;
                    let (muted, notifications) = (model.muted, model.notifications);
                    cx.emit(ChannelEvent::SetNotifications {
                        muted,
                        notifications,
                    });
                }
            }
            Local::Search(terms) => cx.emit(ChannelEvent::Slash(Request::Search(terms))),
            Local::Status(presence) => cx.emit(ChannelEvent::Slash(Request::Status(presence))),
            Local::CollapsePreviews(collapsed) => {
                cx.emit(ChannelEvent::Slash(Request::CollapsePreviews(collapsed)))
            }
            Local::SignOut => cx.emit(ChannelEvent::Slash(Request::SignOut)),
            Local::Code(_) => {}
        }
    }

    /// Takes sync's answers about this channel's commands: suggestions,
    /// and the result of the one running.
    pub fn slash_change(
        &mut self,
        change: &matty_sync::ChangeSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.channel_id() else {
            return;
        };
        match Change::of(change, &channel_id, "") {
            Some(Change::Suggested {
                user_input,
                suggestions,
            }) => {
                let (shown, next) = self.slash.suggested(user_input, suggestions);
                if let Some(user_input) = next {
                    cx.emit(ChannelEvent::Slash(Request::Suggest {
                        channel_id,
                        root_id: SharedString::default(),
                        user_input,
                    }));
                }
                if shown && let Some(query) = self.slash.query().map(str::to_owned) {
                    self.show_commands(&query, cx);
                }
                if shown && self.slash.line().is_some() {
                    crate::slash::show_line(&self.slash, &self.composer, cx);
                }
            }
            Some(Change::Ran { command, outcome }) => {
                let Some(ran) = self.slash.ran(command, outcome) else {
                    return;
                };
                let text = self.composer.read(cx).value(cx);
                if ran.done.is_some_and(|done| text.trim() == done.trim()) {
                    self.clear_command(window, cx);
                }
                if let Some(goto) = ran.goto {
                    cx.emit(ChannelEvent::OpenLink(goto));
                }
                cx.notify();
            }
            Some(Change::Lost) if self.slash.lost() => cx.notify(),
            Some(Change::Lost) | None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{Focusable as _, VisualTestContext};

    use super::*;

    /// #210: typing `/sh` lists `/shortcuts` and `/shrug` in the
    /// composer's autocomplete and asks the server for more; `/me waves`
    /// runs on the server instead of posting, and clears once it ran; an
    /// unknown command's refusal shows above the composer and keeps the
    /// line.
    #[gpui_kit::test]
    fn commands_autocomplete_run_and_show_refusals(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        let channel_id = general.id.clone();
        let (view, cx) = cx.add_window_view(|window, cx| {
            ChannelView::new(Some(general.clone()), "me", window, cx)
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| match event {
                ChannelEvent::Slash(_) | ChannelEvent::Send { .. } => {
                    events.borrow_mut().push(event.clone())
                }
                _ => {}
            })
            .detach();
            let input = view.read(cx).composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        let text = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| view.read(cx).composer.read(cx).value(cx).to_string())
        };
        let ran = |cx: &mut VisualTestContext, command: &str, outcome| {
            let change = matty_sync::ChangeSet::CommandRan {
                channel_id: channel_id.to_string(),
                root_id: String::new(),
                command: command.into(),
                outcome,
            };
            cx.update(|window, cx| {
                view.update(cx, |view, cx| view.slash_change(&change, window, cx))
            });
            cx.run_until_parked();
        };

        // `/sh` lists both commands at once, and asks the server.
        cx.simulate_input("/sh");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let composer = view.read(cx).composer.read(cx);
            assert!(composer.is_autocomplete_open());
            assert_eq!(
                composer
                    .highlighted_suggestion()
                    .map(|row| row.value.to_string()),
                Some("/shortcuts".into())
            );
        });
        cx.simulate_keystrokes("down");
        cx.update(|_, cx| {
            let composer = view.read(cx).composer.read(cx);
            assert_eq!(
                composer
                    .highlighted_suggestion()
                    .map(|row| row.value.to_string()),
                Some("/shrug".into())
            );
        });
        // As in Mattermost, each keystroke asks once.
        assert_eq!(
            events.borrow_mut().drain(..).collect::<Vec<_>>(),
            ["/", "/s", "/sh"].map(|user_input| ChannelEvent::Slash(Request::Suggest {
                channel_id: channel_id.clone(),
                root_id: SharedString::default(),
                user_input: user_input.into(),
            }))
        );
        // The server's own commands join the list.
        let suggested = matty_sync::ChangeSet::CommandSuggestions {
            channel_id: channel_id.to_string(),
            root_id: String::new(),
            user_input: "/sh".into(),
            suggestions: vec![matty_api::models::CommandSuggestion {
                complete: "ship".into(),
                suggestion: "ship".into(),
                description: "Ship a build".into(),
                ..Default::default()
            }]
            .into(),
        };
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.slash_change(&suggested, window, cx))
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(
                    view.slash
                        .suggestions("sh")
                        .iter()
                        .map(|row| row.value.to_string())
                        .collect::<Vec<_>>(),
                    ["/ship", "/shortcuts", "/shrug"]
                );
                view.composer
                    .update(cx, |composer, cx| composer.set_value("", window, cx));
            })
        });

        // `/me waves` goes to the server, not the channel.
        cx.simulate_input("/me waves");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let execute = |command: &str| {
            ChannelEvent::Slash(Request::Execute {
                channel_id: channel_id.clone(),
                root_id: SharedString::default(),
                command: command.into(),
            })
        };
        let sent: Vec<_> = events.borrow_mut().drain(..).collect();
        assert!(sent.contains(&execute("/me waves")), "{sent:?}");
        assert!(
            !sent
                .iter()
                .any(|event| matches!(event, ChannelEvent::Send { .. })),
            "not posted as text: {sent:?}"
        );
        assert_eq!(text(cx), "/me waves", "kept until the server answers");
        ran(cx, "/me waves", Ok(Default::default()));
        assert_eq!(text(cx), "");

        // An unknown command: the server's words show, the line stays.
        cx.simulate_input("/nope");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(events.borrow().contains(&execute("/nope")));
        ran(
            cx,
            "/nope",
            Err("Command with a trigger of 'nope' not found.".into()),
        );
        assert_eq!(text(cx), "/nope");
        cx.update(|_, cx| {
            assert_eq!(
                view.read(cx).slash.error().map(|error| error.to_string()),
                Some("Command with a trigger of 'nope' not found.".into())
            );
        });
        // Drawn above the composer, and gone once the line changes.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_input("x");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(view.read(cx).slash.error(), None));
    }

    /// gpuicn#136: past a command's name, the server is asked about the
    /// whole line and its suggestions open over it; accepting one replaces
    /// the line, and a line that is no longer a command closes them.
    #[gpui_kit::test]
    fn arguments_are_suggested_for_the_whole_line(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        let channel_id = general.id.clone();
        let (view, cx) = cx.add_window_view(|window, cx| {
            ChannelView::new(Some(general.clone()), "me", window, cx)
        });
        let asked = Rc::new(RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let asked = asked.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| {
                if let ChannelEvent::Slash(Request::Suggest { user_input, .. }) = event {
                    asked.borrow_mut().push(user_input.clone());
                }
            })
            .detach();
            let input = view.read(cx).composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        cx.simulate_input("/jira cr");
        cx.run_until_parked();
        assert_eq!(asked.borrow().last().map(String::as_str), Some("/jira cr"));
        cx.update(|_, cx| {
            let composer = view.read(cx).composer.read(cx);
            assert_eq!(composer.suggestion_range(), Some(0..8));
            assert!(
                !composer.is_autocomplete_open(),
                "nothing until the server answers"
            );
        });

        let suggested = matty_sync::ChangeSet::CommandSuggestions {
            channel_id: channel_id.to_string(),
            root_id: String::new(),
            user_input: "/jira cr".into(),
            suggestions: vec![
                matty_api::models::CommandSuggestion {
                    complete: "jira create".into(),
                    suggestion: "create".into(),
                    hint: "[project]".into(),
                    description: "Create an issue".into(),
                    ..Default::default()
                },
                // The line as typed is not offered back.
                matty_api::models::CommandSuggestion {
                    complete: "jira cr".into(),
                    suggestion: "cr".into(),
                    ..Default::default()
                },
            ]
            .into(),
        };
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.slash_change(&suggested, window, cx))
        });
        cx.update(|_, cx| {
            let composer = view.read(cx).composer.read(cx);
            assert!(composer.is_autocomplete_open());
            assert_eq!(
                composer
                    .highlighted_suggestion()
                    .map(|row| row.value.to_string()),
                Some("/jira create".into())
            );
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let composer = view.read(cx).composer.read(cx);
            assert_eq!(composer.value(cx).as_ref(), "/jira create ");
            assert_eq!(
                composer.suggestion_range(),
                Some(0..13),
                "the next argument"
            );
        });
        assert_eq!(
            asked.borrow().last().map(String::as_str),
            Some("/jira create ")
        );

        // Not a command any more: the suggestions close.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.composer.update(cx, |composer, cx| {
                    composer.set_value("hello there", window, cx)
                })
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).composer.read(cx).suggestion_range(), None);
        });
    }

    /// #210: offline, a server command is refused with the offline toast
    /// and the line stays; `/code` still posts, as a code block.
    #[gpui_kit::test]
    fn offline_commands_are_refused_but_code_posts(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let general = crate::preview::channel("general").unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| {
            ChannelView::new(Some(general.clone()), "me", window, cx)
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|window, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event: &ChannelEvent, _| match event {
                ChannelEvent::Slash(_) | ChannelEvent::Send { .. } | ChannelEvent::Offline(_) => {
                    events.borrow_mut().push(event.clone())
                }
                _ => {}
            })
            .detach();
            view.update(cx, |view, _| view.set_offline(true));
            let input = view.read(cx).composer.read(cx).input().clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        cx.simulate_input("/shrug ok");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            events.borrow_mut().drain(..).collect::<Vec<_>>(),
            [ChannelEvent::Offline("run command")]
        );
        assert!(crate::offline::notice("run command").is_some());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.composer.update(cx, |composer, cx| {
                    composer.set_value("/code let x = 1;", window, cx)
                });
                view.send("/code let x = 1;".into(), window, cx);
            })
        });
        let sent: Vec<_> = events.borrow_mut().drain(..).collect();
        assert!(
            matches!(sent.as_slice(), [ChannelEvent::Send { text, .. }] if text == "```\nlet x = 1;\n```"),
            "{sent:?}"
        );
    }
}
