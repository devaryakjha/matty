//! Slash commands (#210, PLAN §4.3).
//!
//! `/` at the start of a composer opens its autocomplete with the commands:
//! Mattermost's own, which Matty lists at once, and whatever the server
//! suggests for the line, which adds plugins' and integrations' commands
//! and, for a whole command name, its subcommands and dynamic arguments.
//!
//! Enter on a command line runs it instead of posting it. Commands Matty
//! has an action for run here ([`Local`]); the rest go to the server's
//! `commands/execute` through sync. The line stays in the composer until
//! the server answers: an answer clears it, and a refusal (such as an
//! unknown command) shows in the server's words above the composer, where
//! the user can fix the line. An ephemeral answer arrives as an ephemeral
//! message through the interactive session (#211), a dialog through
//! `open_dialog`, and a `goto_location` opens like a link in a message.

use std::collections::HashMap;

use gpui_icons::{LucideIcon, lucide};
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};
use matty_api::models::CommandSuggestion;

use crate::sidebar::Presence;
use crate::style::{self, TypeStyleExt as _};
use crate::ui::composer::ComposerSuggestion;
use crate::ui::theme::UiTheme;

/// The most rows the autocomplete lists.
const LIMIT: usize = 24;

/// One of Mattermost's own commands, for the list shown before the server
/// answers (and without one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub trigger: &'static str,
    /// Its arguments, such as `[message]`.
    pub hint: &'static str,
    pub description: &'static str,
}

const fn builtin(trigger: &'static str, hint: &'static str, description: &'static str) -> Builtin {
    Builtin {
        trigger,
        hint,
        description,
    }
}

/// Mattermost's own commands, in its words.
pub const BUILTINS: &[Builtin] = &[
    builtin("away", "", "Set your status to away"),
    builtin("code", "[text]", "Display text as a code block"),
    builtin("collapse", "", "Collapse link previews"),
    builtin(
        "dnd",
        "",
        "Do not disturb: no desktop or push notifications",
    ),
    builtin(
        "echo",
        "[message] [delay]",
        "Echo back text from your account",
    ),
    builtin("expand", "", "Expand link previews"),
    builtin("groupmsg", "@[usernames] [message]", "Send a group message"),
    builtin("header", "[text]", "Edit the channel header"),
    builtin("help", "", "Open the Mattermost help page"),
    builtin(
        "invite",
        "@[username] ~[channel]",
        "Invite a user to a channel",
    ),
    builtin("join", "~[channel]", "Join a channel"),
    builtin("kick", "@[username]", "Remove a member from the channel"),
    builtin("leave", "", "Leave the current channel"),
    builtin("logout", "", "Sign out of this server"),
    builtin("me", "[message]", "Do an action"),
    builtin("msg", "@[username] [message]", "Send a direct message"),
    builtin(
        "mute",
        "~[channel]",
        "Mute the current channel or the one named",
    ),
    builtin("offline", "", "Set your status to offline"),
    builtin("online", "", "Set your status to online"),
    builtin("purpose", "[text]", "Edit the channel purpose"),
    builtin("remove", "@[username]", "Remove a member from the channel"),
    builtin("rename", "[text]", "Rename the channel"),
    builtin("search", "[text]", "Search messages"),
    builtin("settings", "", "Open preferences"),
    builtin("shortcuts", "", "Show keyboard shortcuts"),
    builtin("shrug", "[message]", "Add ¯\\_(ツ)_/¯ to your message"),
];

/// Where a command was typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Channel,
    Thread,
}

/// A command Matty runs itself, with a Matty action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Local {
    /// The keyboard shortcut sheet.
    Shortcuts,
    /// Preferences.
    Settings,
    /// Search for the text, or open search.
    Search(String),
    /// The channel's "Leave channel" confirmation.
    Leave,
    /// Mute or unmute the channel.
    Mute,
    /// The channel's "Edit header" dialog.
    Header,
    /// The channel's "Edit purpose" dialog.
    Purpose,
    /// Set the user's status.
    Status(Presence),
    /// Post the text as a code block, as any message is sent.
    Code(String),
    /// Collapse (`true`) or expand link previews.
    CollapsePreviews(bool),
    /// Sign out of the server.
    SignOut,
}

/// What a line asks for when sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// Not a command: a message.
    Message,
    /// A command Matty runs.
    Local(Local),
    /// A command for the server, as typed.
    Server(String),
}

/// What the line is: a command starts with `/` and a name. `/ x` and `//x`
/// are messages, as are lines whose command needs what the place lacks.
pub fn parse(text: &str, place: Place) -> Line {
    let line = text.trim();
    let Some(rest) = line.strip_prefix('/') else {
        return Line::Message;
    };
    let (trigger, args) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(trigger, args)| (trigger, args.trim()));
    if trigger.is_empty() || trigger.starts_with('/') {
        return Line::Message;
    }
    let channel = place == Place::Channel;
    let local = match trigger.to_lowercase().as_str() {
        "shortcuts" => Some(Local::Shortcuts),
        "settings" => Some(Local::Settings),
        "search" => Some(Local::Search(args.to_owned())),
        "away" => Some(Local::Status(Presence::Away)),
        "online" => Some(Local::Status(Presence::Online)),
        "dnd" => Some(Local::Status(Presence::Dnd)),
        "offline" => Some(Local::Status(Presence::Offline)),
        "logout" => Some(Local::SignOut),
        "collapse" => Some(Local::CollapsePreviews(true)),
        "expand" => Some(Local::CollapsePreviews(false)),
        "code" if !args.is_empty() => Some(Local::Code(code_block(rest, trigger))),
        // The channel's own dialogs; with text, or in a thread, the server
        // does it.
        "leave" if channel && args.is_empty() => Some(Local::Leave),
        "mute" if channel && args.is_empty() => Some(Local::Mute),
        "header" if channel && args.is_empty() => Some(Local::Header),
        "purpose" if channel && args.is_empty() => Some(Local::Purpose),
        _ => None,
    };
    match local {
        Some(local) => Line::Local(local),
        None => Line::Server(line.to_owned()),
    }
}

/// `/code`'s text as a fenced block, keeping its lines and indents.
fn code_block(rest: &str, trigger: &str) -> String {
    let text = rest[trigger.len()..]
        .strip_prefix([' ', '\t'])
        .unwrap_or(&rest[trigger.len()..]);
    let text = text.trim_end();
    let text = text.strip_prefix('\n').unwrap_or(text);
    format!("```\n{text}\n```")
}

/// A request a view makes of its window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Ask the server what may follow `user_input`.
    Suggest {
        channel_id: SharedString,
        root_id: SharedString,
        user_input: String,
    },
    /// Run the line on the server.
    Execute {
        channel_id: SharedString,
        root_id: SharedString,
        command: String,
    },
    /// Set the user's status.
    Status(Presence),
    /// Collapse (`true`) or expand link previews.
    CollapsePreviews(bool),
    /// Search for the text, or open search.
    Search(String),
    /// Sign out of the server.
    SignOut,
}

/// What a server answer means for the view that ran the command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    /// The command went: the composer clears, if it still holds it.
    pub done: Option<String>,
    /// A place to open.
    pub goto: Option<SharedString>,
}

/// A view's slash command state: the server's suggestions for the lines it
/// asked about, the line running, and the refusal shown.
#[derive(Debug, Default)]
pub struct Slash {
    /// The server's suggestions by the line asked about; `None` while asked.
    server: HashMap<String, Option<Vec<CommandSuggestion>>>,
    /// The `/` query the autocomplete shows now.
    query: Option<String>,
    /// The line waiting for the server.
    running: Option<String>,
    /// The server's refusal of `running`, and the line refused.
    error: Option<(String, SharedString)>,
}

impl Slash {
    /// Forgets what the server said, for another channel or thread.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The line waiting for the server.
    pub fn running(&self) -> Option<&str> {
        self.running.as_deref()
    }

    /// What follows `/` in the composer while its autocomplete is open.
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// The refusal shown above the composer.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref().map(|(_, error)| error)
    }

    /// The composer shows `query` after `/`. Returns the lines to ask the
    /// server about: the name so far, and once it names a command with
    /// arguments, what may follow it. Offline (`online` false) there is no
    /// one to ask, and they are asked once back.
    pub fn typing(&mut self, query: &str, online: bool) -> Vec<String> {
        self.query = Some(query.to_owned());
        if !online {
            return Vec::new();
        }
        let mut ask = vec![format!("/{query}")];
        if self.takes_arguments(query) {
            ask.push(format!("/{query} "));
        }
        ask.retain(|line| !self.server.contains_key(line));
        for line in &ask {
            self.server.insert(line.clone(), None);
        }
        ask
    }

    /// The autocomplete closed.
    pub fn closed(&mut self) {
        self.query = None;
    }

    /// Whether `trigger` names a command with arguments, as far as Matty
    /// or the server has said.
    fn takes_arguments(&self, trigger: &str) -> bool {
        let trigger = trigger.to_lowercase();
        if trigger.is_empty() {
            return false;
        }
        let builtin = BUILTINS
            .iter()
            .any(|command| command.trigger == trigger && !command.hint.is_empty());
        let server = self
            .server
            .values()
            .flatten()
            .flatten()
            .any(|found| found.complete.to_lowercase() == trigger && !found.hint.is_empty());
        builtin || server
    }

    /// Takes the server's suggestions for `user_input`. Returns whether the
    /// autocomplete shows them now, and any line to ask about next: what
    /// follows a command the answer says takes arguments.
    pub fn suggested(
        &mut self,
        user_input: &str,
        suggestions: &[CommandSuggestion],
    ) -> (bool, Option<String>) {
        if !self.server.contains_key(user_input) {
            return (false, None);
        }
        self.server
            .insert(user_input.to_owned(), Some(suggestions.to_vec()));
        let Some(query) = self.query.clone() else {
            return (false, None);
        };
        let shown = user_input == format!("/{query}") || user_input == format!("/{query} ");
        let next = format!("/{query} ");
        let follow = (shown && !self.server.contains_key(&next) && self.takes_arguments(&query))
            .then(|| {
                self.server.insert(next.clone(), None);
                next
            });
        (shown, follow)
    }

    /// The autocomplete rows for `query` after `/`: Matty's list of
    /// Mattermost's commands and the server's names, then, for a whole
    /// command name, its subcommands and arguments.
    pub fn suggestions(&self, query: &str) -> Vec<ComposerSuggestion> {
        let typed = query.to_lowercase();
        let mut names: Vec<(String, String, String)> = BUILTINS
            .iter()
            .filter(|command| command.trigger.starts_with(&typed))
            .map(|command| {
                (
                    command.trigger.to_owned(),
                    command.hint.to_owned(),
                    command.description.to_owned(),
                )
            })
            .collect();
        let server = |line: String| {
            self.server
                .get(&line)
                .cloned()
                .flatten()
                .unwrap_or_default()
        };
        for found in server(format!("/{query}")) {
            let name = found.complete.trim().to_owned();
            if name.is_empty() || name.contains(char::is_whitespace) {
                continue;
            }
            match names.iter_mut().find(|(known, ..)| *known == name) {
                // The server's words for its own command win.
                Some(known) => {
                    if !found.description.is_empty() {
                        known.2 = found.description.clone();
                    }
                }
                None => names.push((name, found.hint.clone(), found.description.clone())),
            }
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let rows = names.into_iter().map(|(name, hint, description)| {
            row(format!("/{name}"), &hint, &description, LucideIcon::Slash)
        });
        let arguments = server(format!("/{query} "))
            .into_iter()
            .filter(|found| !found.complete.trim().is_empty())
            .map(|found| {
                row(
                    format!("/{}", found.complete.trim_end()),
                    &found.hint,
                    &found.description,
                    LucideIcon::CornerDownRight,
                )
            });
        let mut rows: Vec<ComposerSuggestion> = rows.chain(arguments).collect();
        rows.dedup_by(|a, b| a.value == b.value);
        rows.truncate(LIMIT);
        rows
    }

    /// The line went to the server.
    pub fn started(&mut self, line: &str) {
        self.running = Some(line.to_owned());
        self.error = None;
    }

    /// The composer's text changed: a refusal goes once the line does.
    /// Returns whether it went.
    pub fn edited(&mut self, text: &str) -> bool {
        if self
            .error
            .as_ref()
            .is_some_and(|(line, _)| line.trim() != text.trim())
        {
            self.error = None;
            return true;
        }
        false
    }

    /// The server answered a command run where this view is. Returns
    /// `None` when it was not this view's.
    pub fn ran(
        &mut self,
        command: &str,
        outcome: &Result<matty_api::models::CommandResponse, String>,
    ) -> Option<Ran> {
        if self.running.as_deref() != Some(command) {
            return None;
        }
        self.running = None;
        Some(match outcome {
            Ok(response) => Ran {
                done: Some(command.to_owned()),
                goto: (!response.goto_location.is_empty())
                    .then(|| response.goto_location.clone().into()),
            },
            Err(error) => {
                self.error = Some((command.to_owned(), error.clone().into()));
                Ran::default()
            }
        })
    }

    /// Offline, the command never reached the server; the window's toast
    /// says so. Returns whether one was waiting.
    pub fn lost(&mut self) -> bool {
        self.running.take().is_some()
    }
}

fn row(value: String, hint: &str, description: &str, icon: LucideIcon) -> ComposerSuggestion {
    let detail = match (hint.is_empty(), description.is_empty()) {
        (true, true) => None,
        (false, true) => Some(hint.to_owned()),
        (true, false) => Some(description.to_owned()),
        (false, false) => Some(format!("{hint} · {description}")),
    };
    let row = ComposerSuggestion::new(value.clone(), value).icon(icon);
    match detail {
        Some(detail) => row.description(detail),
        None => row,
    }
}

/// What a change set means for a view showing `channel_id`, in the thread
/// `root_id` (empty for the channel).
pub enum Change<'a> {
    Suggested {
        user_input: &'a str,
        suggestions: &'a [CommandSuggestion],
    },
    Ran {
        command: &'a str,
        outcome: &'a Result<matty_api::models::CommandResponse, String>,
    },
    /// Offline, a command did not go.
    Lost,
}

impl<'a> Change<'a> {
    /// The part of `change` for the view at `channel_id` and `root_id`.
    pub fn of(change: &'a matty_sync::ChangeSet, channel_id: &str, root_id: &str) -> Option<Self> {
        use matty_sync::{ChangeSet, CommandError};
        match change {
            ChangeSet::CommandSuggestions {
                channel_id: channel,
                root_id: root,
                user_input,
                suggestions,
            } if channel == channel_id && root == root_id => Some(Self::Suggested {
                user_input,
                suggestions,
            }),
            ChangeSet::CommandRan {
                channel_id: channel,
                root_id: root,
                command,
                outcome,
            } if channel == channel_id && root == root_id => Some(Self::Ran { command, outcome }),
            ChangeSet::Failed(CommandError::Offline {
                action: "run command",
            }) => Some(Self::Lost),
            _ => None,
        }
    }
}

/// The ephemeral message a command's answer carries outside the socket
/// (an Apps framework command's `ephemeral_text`), as the interactive
/// session keeps ephemeral messages.
pub fn ephemeral(change: &matty_sync::ChangeSet) -> Option<matty_api::models::Post> {
    let matty_sync::ChangeSet::CommandRan {
        channel_id,
        root_id,
        command,
        outcome: Ok(response),
    } = change
    else {
        return None;
    };
    if response.ephemeral_text.is_empty() {
        return None;
    }
    let now = jiff::Timestamp::now().as_millisecond();
    Some(matty_api::models::Post {
        id: format!("command-{now}-{}", command.len()),
        channel_id: channel_id.clone(),
        root_id: root_id.clone(),
        message: response.ephemeral_text.clone(),
        kind: crate::interactive::EPHEMERAL.to_owned(),
        create_at: now,
        update_at: now,
        ..Default::default()
    })
}

/// The sync command for a request that only needs the server: asking for
/// suggestions, running a command, or setting the user's status.
pub fn sync_command(live: &crate::live::Live, request: &Request) -> Option<matty_sync::Command> {
    let team_id = live.team().unwrap_or_default().to_owned();
    Some(match request {
        Request::Suggest {
            channel_id,
            root_id,
            user_input,
        } => {
            // Suggestions are for a team; with none there is nothing to ask.
            if team_id.is_empty() {
                return None;
            }
            matty_sync::Command::SuggestCommands {
                team_id,
                channel_id: channel_id.to_string(),
                root_id: root_id.to_string(),
                user_input: user_input.clone(),
            }
        }
        Request::Execute {
            channel_id,
            root_id,
            command,
        } => matty_sync::Command::ExecuteCommand {
            team_id,
            channel_id: channel_id.to_string(),
            root_id: root_id.to_string(),
            command: command.clone(),
        },
        Request::Status(presence) => matty_sync::Command::SetStatus {
            status: crate::live::status(*presence).into(),
            dnd_end_time: None,
        },
        Request::Search(_) | Request::SignOut | Request::CollapsePreviews(_) => return None,
    })
}

/// The refusal above the composer, as the server worded it.
pub fn error_line(id: &'static str, error: Option<&SharedString>, cx: &App) -> Option<AnyElement> {
    let error = error?;
    let theme = UiTheme::read(cx).clone();
    Some(
        div()
            .id(id)
            .role(gpui_kit::Role::Alert)
            .aria_label(error.clone())
            .flex()
            .items_center()
            .gap(theme.space(1.5))
            .pb(theme.space(1.5))
            .type_style(style::META, &theme)
            .text_color(theme.colors.destructive)
            .child(
                lucide(LucideIcon::CircleAlert)
                    .size(theme.space(3.5))
                    .flex_shrink_0()
                    .text_color(theme.colors.destructive),
            )
            .child(div().min_w_0().child(error.clone()))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(complete: &str, hint: &str, description: &str) -> CommandSuggestion {
        CommandSuggestion {
            complete: complete.into(),
            suggestion: complete.rsplit(' ').next().unwrap_or_default().into(),
            hint: hint.into(),
            description: description.into(),
            icon_data: String::new(),
        }
    }

    fn values(rows: &[ComposerSuggestion]) -> Vec<&str> {
        rows.iter().map(|row| row.value.as_ref()).collect()
    }

    #[test]
    fn lines_are_messages_local_or_for_the_server() {
        use Place::{Channel, Thread};
        assert_eq!(parse("hello", Channel), Line::Message);
        assert_eq!(parse("/ hello", Channel), Line::Message);
        assert_eq!(parse("//not a command", Channel), Line::Message);
        assert_eq!(
            parse("/me waves", Channel),
            Line::Server("/me waves".into())
        );
        assert_eq!(parse("  /shrug  ", Thread), Line::Server("/shrug".into()));
        assert_eq!(parse("/shortcuts", Thread), Line::Local(Local::Shortcuts));
        assert_eq!(parse("/Settings", Channel), Line::Local(Local::Settings));
        assert_eq!(
            parse("/search deploy notes", Channel),
            Line::Local(Local::Search("deploy notes".into()))
        );
        assert_eq!(
            parse("/dnd", Channel),
            Line::Local(Local::Status(Presence::Dnd))
        );
        assert_eq!(
            parse("/expand", Channel),
            Line::Local(Local::CollapsePreviews(false))
        );
        assert_eq!(parse("/logout", Channel), Line::Local(Local::SignOut));
        // The channel's dialogs open with no text; with text the server
        // sets it, and a thread has no such dialogs.
        assert_eq!(parse("/header", Channel), Line::Local(Local::Header));
        assert_eq!(
            parse("/header Release day", Channel),
            Line::Server("/header Release day".into())
        );
        assert_eq!(parse("/leave", Thread), Line::Server("/leave".into()));
        assert_eq!(parse("/mute", Channel), Line::Local(Local::Mute));
        assert_eq!(
            parse("/code fn main() {}\n    run();", Channel),
            Line::Local(Local::Code("```\nfn main() {}\n    run();\n```".into()))
        );
        assert_eq!(parse("/code", Channel), Line::Server("/code".into()));
    }

    #[test]
    fn names_list_at_once_and_the_server_adds_its_own() {
        let mut slash = Slash::default();
        assert_eq!(slash.typing("sh", true), ["/sh"]);
        assert_eq!(
            values(&slash.suggestions("sh")),
            ["/shortcuts", "/shrug"],
            "Mattermost's own list before the server answers"
        );
        let (shown, next) = slash.suggested(
            "/sh",
            &[
                suggestion("shrug", "[message]", "Shrug, from the server"),
                suggestion("ship", "[env]", "Ship it"),
            ],
        );
        assert!(shown);
        assert_eq!(next, None);
        let rows = slash.suggestions("sh");
        assert_eq!(values(&rows), ["/ship", "/shortcuts", "/shrug"]);
        assert_eq!(
            rows[2].description.as_deref(),
            Some("[message] · Shrug, from the server")
        );
        // Asked once per line, and not offline.
        assert!(slash.typing("sh", true).is_empty());
        assert!(slash.typing("shr", false).is_empty());
        assert_eq!(slash.typing("shr", true), ["/shr"]);
    }

    #[test]
    fn a_whole_command_name_lists_its_arguments() {
        let mut slash = Slash::default();
        assert_eq!(slash.typing("jira", true), ["/jira"]);
        let (shown, next) = slash.suggested("/jira", &[suggestion("jira", "[command]", "Jira")]);
        assert!(shown);
        assert_eq!(next.as_deref(), Some("/jira "), "it takes arguments");
        slash.suggested(
            "/jira ",
            &[
                suggestion("jira create", "[text]", "Create a new Issue"),
                suggestion("jira connect", "", "Connect your account"),
            ],
        );
        assert_eq!(
            values(&slash.suggestions("jira")),
            ["/jira", "/jira create", "/jira connect"]
        );
        // A built-in name with arguments asks for them straight away.
        assert_eq!(slash.typing("me", true), ["/me", "/me "]);
        // An answer for a line no longer typed is kept, not shown.
        slash.typing("x", true);
        assert_eq!(slash.suggested("/me", &[]), (false, None));
        assert_eq!(slash.suggested("/never-asked", &[]), (false, None));
    }

    #[test]
    fn a_refusal_shows_until_the_line_changes() {
        let mut slash = Slash::default();
        slash.started("/nope");
        assert_eq!(slash.running(), Some("/nope"));
        assert_eq!(
            slash.ran("/other", &Err("not mine".into())),
            None,
            "another view's command"
        );
        assert_eq!(
            slash.ran(
                "/nope",
                &Err("Command with a trigger of 'nope' not found.".into())
            ),
            Some(Ran::default())
        );
        assert_eq!(
            slash.error().map(|e| e.as_ref()),
            Some("Command with a trigger of 'nope' not found.")
        );
        assert!(!slash.edited("/nope "));
        assert!(slash.edited("/nop"));
        assert_eq!(slash.error(), None);
        slash.started("/go");
        let goto = matty_api::models::CommandResponse {
            goto_location: "https://chat.example.com/t/channels/x".into(),
            ..Default::default()
        };
        assert_eq!(
            slash.ran("/go", &Ok(goto)),
            Some(Ran {
                done: Some("/go".into()),
                goto: Some("https://chat.example.com/t/channels/x".into()),
            })
        );
        slash.started("/away");
        assert!(slash.lost());
        assert!(!slash.lost());
    }

    #[test]
    fn an_answer_with_ephemeral_text_becomes_an_ephemeral_message() {
        let ran = |text: &str| matty_sync::ChangeSet::CommandRan {
            channel_id: "c1".into(),
            root_id: "r1".into(),
            command: "/app".into(),
            outcome: Ok(matty_api::models::CommandResponse {
                ephemeral_text: text.into(),
                ..Default::default()
            }),
        };
        let post = ephemeral(&ran("Only you see this")).unwrap();
        assert!(crate::interactive::is_ephemeral(&post));
        assert_eq!(
            (post.channel_id.as_str(), post.root_id.as_str()),
            ("c1", "r1")
        );
        assert_eq!(post.message, "Only you see this");
        assert!(ephemeral(&ran("")).is_none());
    }
}
