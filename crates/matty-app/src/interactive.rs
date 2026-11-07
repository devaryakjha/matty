//! Interactive message attachments and ephemeral messages (#211).
//!
//! Integrations put buttons and menus on a message attachment. Choosing
//! one calls `posts/{post_id}/actions/{action_id}` through sync; the button
//! or menu shows it is waiting until the server answers, and a refusal is a
//! toast. The integration answers by changing the post, or with an
//! ephemeral message: a post only this user sees, which the server never
//! stores and Matty keeps in memory only, marked "Only visible to you" until
//! dismissed.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use matty_api::models::{Post as ApiPost, PostAction, User};

use crate::embeds::Embed;
use crate::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::ui::select::{Select, SelectEvent, SelectItem, SelectState};
use crate::ui::spinner::Spinner;
use crate::ui::theme::UiTheme;

/// The post type of an ephemeral message.
pub const EPHEMERAL: &str = "system_ephemeral";

/// Whether `post` is an ephemeral message.
pub fn is_ephemeral(post: &ApiPost) -> bool {
    post.kind == EPHEMERAL
}

/// A button or menu on an attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionCard {
    pub id: SharedString,
    /// The button's label, or the menu's placeholder.
    pub name: SharedString,
    /// `primary`, `danger`, ... as the integration styled it.
    pub style: SharedString,
    pub disabled: bool,
    pub cookie: SharedString,
    /// Set for a menu.
    pub menu: Option<ActionMenu>,
    /// Sent and not yet answered: the control waits and takes no input.
    pub pending: bool,
}

/// A menu's options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionMenu {
    pub source: MenuSource,
    /// `(value, label)`, in the order shown.
    pub options: Vec<(SharedString, SharedString)>,
    /// The value it starts on; empty for none.
    pub default: SharedString,
}

/// Where a menu's options come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuSource {
    /// The integration listed them.
    Static,
    /// The people Matty knows; the value is a user id.
    Users,
    /// The channels the user is in; the value is a channel id.
    Channels,
}

impl ActionCard {
    pub fn from_api(action: &PostAction) -> Self {
        let menu = action.is_select().then(|| ActionMenu {
            source: match action.data_source.as_str() {
                "users" => MenuSource::Users,
                "channels" => MenuSource::Channels,
                _ => MenuSource::Static,
            },
            options: action
                .options
                .iter()
                .map(|option| (option.value.clone().into(), option.text.clone().into()))
                .collect(),
            default: action.default_option.clone().into(),
        });
        Self {
            id: action.id.clone().into(),
            name: action.name.clone().into(),
            style: action.style.clone().into(),
            disabled: action.disabled,
            cookie: action.cookie.clone().into(),
            menu,
            pending: false,
        }
    }

    fn variant(&self) -> ButtonVariant {
        match self.style.as_ref() {
            "primary" => ButtonVariant::Default,
            "danger" => ButtonVariant::Destructive,
            _ => ButtonVariant::Outline,
        }
    }

    /// The colour an integration gave the button: `good`, `success` or
    /// `warning`, or its own like `#3f51b5`. The others are variants.
    fn tint(&self, theme: &UiTheme) -> Option<gpui_kit::Rgba> {
        let style = self.style.trim();
        match style.to_ascii_lowercase().as_str() {
            "good" | "success" => Some(theme.colors.success),
            "warning" => Some(theme.colors.warning),
            _ if style.starts_with('#') => gpui_kit::Rgba::try_from(style).ok(),
            _ => None,
        }
    }
}

/// A button pressed or a menu option picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionChoice {
    pub post_id: SharedString,
    pub action_id: SharedString,
    /// The option picked; `None` for a button.
    pub selected: Option<SharedString>,
    pub cookie: SharedString,
}

/// Runs a choice.
pub type ActionHandler = Rc<dyn Fn(ActionChoice, &mut Window, &mut App)>;

/// The actions of `post`'s attachments.
fn actions_of(embeds: &[Embed]) -> impl Iterator<Item = &ActionCard> {
    embeds.iter().flat_map(|embed| match embed {
        Embed::Attachment(card) => card.actions.as_slice(),
        _ => &[],
    })
}

fn actions_of_mut(embeds: &mut [Embed]) -> impl Iterator<Item = &mut ActionCard> {
    embeds.iter_mut().flat_map(|embed| match embed {
        Embed::Attachment(card) => card.actions.as_mut_slice(),
        _ => &mut [],
    })
}

/// Fills in what the view cannot know from a post alone: which actions
/// wait for the server, and the people and channels a `users` or
/// `channels` menu lists.
pub fn fill(
    posts: &mut [crate::channel::Post],
    pending: &HashSet<(String, String)>,
    people: &[(SharedString, SharedString)],
    channels: &[(SharedString, SharedString)],
) {
    for post in posts {
        let post_id = post.id.to_string();
        for action in actions_of_mut(&mut post.embeds) {
            action.pending = pending.contains(&(post_id.clone(), action.id.to_string()));
            if let Some(menu) = &mut action.menu {
                match menu.source {
                    MenuSource::Static => {}
                    MenuSource::Users => menu.options = people.to_vec(),
                    MenuSource::Channels => menu.options = channels.to_vec(),
                }
            }
        }
    }
}

/// The people a `users` menu offers: `(user id, name)`, by name.
pub fn people(
    users: &HashMap<String, User>,
    names: crate::settings::NameFormat,
) -> Vec<(SharedString, SharedString)> {
    let mut people: Vec<(SharedString, SharedString)> = users
        .values()
        .filter(|user| user.delete_at == 0)
        .map(|user| {
            (
                user.id.clone().into(),
                crate::transcript::display_name_as(user, names).into(),
            )
        })
        .collect();
    people.sort_by_key(|(_, name)| name.to_lowercase());
    people
}

/// The channels a `channels` menu offers: the public and private ones the
/// user is in, from the sidebar's `sections`, as `(channel id, name)`, by
/// name.
pub fn channels<'a>(
    sections: impl Iterator<Item = &'a matty_sync::SidebarSection>,
) -> Vec<(SharedString, SharedString)> {
    let mut channels: Vec<(SharedString, SharedString)> = Vec::new();
    for channel in sections
        .flat_map(|section| &section.channels)
        .filter(|channel| matches!(channel.kind.as_str(), "O" | "P"))
    {
        if !channels.iter().any(|(id, _)| id.as_ref() == channel.id) {
            channels.push((
                channel.id.clone().into(),
                channel.display_name.clone().into(),
            ));
        }
    }
    channels.sort_by_key(|(_, name)| name.to_lowercase());
    channels
}

/// `posts` with the ephemeral messages for `channel_id` among them, by
/// time; `None` when there are none, so the caller keeps its own slice.
pub fn with_ephemeral(
    posts: Option<&[ApiPost]>,
    ephemeral: &[ApiPost],
    channel_id: &str,
) -> Option<Vec<ApiPost>> {
    let posts = posts?;
    let mine: Vec<&ApiPost> = ephemeral
        .iter()
        .filter(|post| post.channel_id == channel_id)
        .collect();
    if mine.is_empty() {
        return None;
    }
    let mut merged: Vec<ApiPost> = posts.to_vec();
    merged.extend(mine.into_iter().cloned());
    merged.sort_by_key(|post| post.create_at);
    Some(merged)
}

/// A thread's posts, root first, with the ephemeral replies under its root
/// among them by time: with collapsed reply threads on, the channel leaves
/// them out, so the thread is where they show.
pub fn with_thread_ephemeral(
    posts: &std::sync::Arc<[ApiPost]>,
    ephemeral: &[ApiPost],
) -> std::sync::Arc<[ApiPost]> {
    let Some(root) = posts.first() else {
        return posts.clone();
    };
    let replies: Vec<&ApiPost> = ephemeral
        .iter()
        .filter(|post| post.root_id == root.id && !posts.iter().any(|kept| kept.id == post.id))
        .collect();
    if replies.is_empty() {
        return posts.clone();
    }
    let mut merged: Vec<ApiPost> = posts[1..].to_vec();
    merged.extend(replies.into_iter().cloned());
    merged.sort_by_key(|post| post.create_at);
    std::iter::once(root.clone()).chain(merged).collect()
}

/// Keeps the ephemeral messages up to date with one from the server: a new
/// or changed one replaces any with its id, and a removed one (`delete_at`
/// set) goes. Returns whether anything changed.
pub fn take_ephemeral(ephemeral: &mut Vec<ApiPost>, post: &ApiPost) -> bool {
    let before = ephemeral.len();
    ephemeral.retain(|kept| kept.id != post.id);
    if post.delete_at == 0 {
        let mut post = post.clone();
        // The server does not always stamp one; it shows now.
        if post.create_at == 0 {
            post.create_at = jiff::Timestamp::now().as_millisecond();
        }
        ephemeral.push(post);
        return true;
    }
    ephemeral.len() != before
}

/// What the session keeps for interactive messages: the ephemeral
/// messages, in memory only, and the actions waiting for the server.
#[derive(Debug, Default)]
pub struct Session {
    ephemeral: Vec<ApiPost>,
    pending: HashSet<(String, String)>,
}

/// What a change set did to the interactive messages.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    /// The open channel's view must redraw.
    pub redraw: bool,
    /// The server refused an action, in its words, for a toast.
    pub refused: Option<SharedString>,
    /// An action could not reach the server.
    pub offline: bool,
}

impl Session {
    /// The ephemeral messages kept.
    pub fn ephemeral(&self) -> &[ApiPost] {
        &self.ephemeral
    }

    /// The actions waiting for the server, by post and action id.
    pub fn pending(&self) -> &HashSet<(String, String)> {
        &self.pending
    }

    /// Marks which of `posts`' actions wait for the server, and lists the
    /// people and the channels (from the sidebar's `sections`) that `users`
    /// and `channels` menus offer.
    pub fn fill<'a>(
        &self,
        posts: &mut [crate::channel::Post],
        users: &HashMap<String, User>,
        names: crate::settings::NameFormat,
        sections: impl Iterator<Item = &'a matty_sync::SidebarSection>,
    ) {
        let sources: HashSet<MenuSource> = posts
            .iter()
            .flat_map(|post| actions_of(&post.embeds))
            .filter_map(|action| action.menu.as_ref().map(|menu| menu.source))
            .collect();
        let people = if sources.contains(&MenuSource::Users) {
            people(users, names)
        } else {
            Vec::new()
        };
        let channels = if sources.contains(&MenuSource::Channels) {
            channels(sections)
        } else {
            Vec::new()
        };
        fill(posts, &self.pending, &people, &channels);
    }

    /// Marks `choice` as sent: its control waits until the server answers.
    pub fn sent(&mut self, choice: &ActionChoice) {
        self.pending
            .insert((choice.post_id.to_string(), choice.action_id.to_string()));
    }

    /// Forgets a dismissed ephemeral message. Returns whether it was kept.
    pub fn dismiss(&mut self, post_id: &str) -> bool {
        let before = self.ephemeral.len();
        self.ephemeral.retain(|post| post.id != post_id);
        self.ephemeral.len() != before
    }

    /// Folds in an ephemeral message or an action's answer; `open` is the
    /// channel on screen.
    pub fn apply(&mut self, change: &matty_sync::ChangeSet, open: Option<&str>) -> Applied {
        use matty_sync::{ChangeSet, CommandError};
        let mut applied = Applied::default();
        match change {
            ChangeSet::Ephemeral(post) => {
                applied.redraw =
                    take_ephemeral(&mut self.ephemeral, post) && open == Some(&post.channel_id);
            }
            ChangeSet::PostActionDone { post_id, action_id } => {
                applied.redraw = self.pending.remove(&(post_id.clone(), action_id.clone()));
            }
            ChangeSet::Failed(CommandError::Server {
                action: "run action",
                target,
                error,
            }) => {
                let before = self.pending.len();
                self.pending.retain(|(post_id, _)| post_id != target);
                applied.redraw = self.pending.len() != before;
                // Every window hears it; the one that sent it says so.
                if applied.redraw {
                    applied.refused = Some(error.clone().into());
                }
            }
            ChangeSet::Failed(CommandError::Offline {
                action: "run action",
            }) => {
                // Sync does not say which: none of them got there.
                applied.redraw = !self.pending.is_empty();
                applied.offline = applied.redraw;
                self.pending.clear();
            }
            _ => {}
        }
        applied
    }
}

/// The menus' retained states, by post and action, for one view.
#[derive(Default)]
pub struct Menus {
    states: HashMap<(SharedString, SharedString), (Vec<SharedString>, Entity<SelectState>)>,
}

impl Menus {
    /// The state for a menu, once [`Menus::sync`] has made it.
    fn get(
        &self,
        post_id: &SharedString,
        action_id: &SharedString,
    ) -> Option<&Entity<SelectState>> {
        self.states
            .get(&(post_id.clone(), action_id.clone()))
            .map(|(_, state)| state)
    }

    /// Makes a state for each menu in `posts` that has none, or whose
    /// options changed, and drops the rest. `on_choice` runs when the
    /// user picks an option.
    pub fn sync<T: 'static>(
        &mut self,
        posts: &[crate::channel::Post],
        window: &mut Window,
        cx: &mut Context<T>,
        on_choice: impl Fn(&mut T, ActionChoice, &mut Context<T>) + Clone + 'static,
    ) {
        let mut seen = HashSet::new();
        for post in posts {
            for action in actions_of(&post.embeds) {
                let Some(menu) = &action.menu else { continue };
                let key = (post.id.clone(), action.id.clone());
                let values: Vec<SharedString> = menu
                    .options
                    .iter()
                    .map(|(value, _)| value.clone())
                    .collect();
                seen.insert(key.clone());
                if self
                    .states
                    .get(&key)
                    .is_some_and(|(known, _)| *known == values)
                {
                    continue;
                }
                let items: Vec<SelectItem> = menu
                    .options
                    .iter()
                    .map(|(value, label)| SelectItem::new(value.clone(), label.clone()))
                    .collect();
                let default = (!menu.default.is_empty()).then(|| menu.default.clone());
                let state = cx.new(|cx| {
                    let mut state = SelectState::new(items, window, cx);
                    state.set_value(default, window, cx);
                    state
                });
                let choice = ActionChoice {
                    post_id: post.id.clone(),
                    action_id: action.id.clone(),
                    selected: None,
                    cookie: action.cookie.clone(),
                };
                let on_choice = on_choice.clone();
                cx.subscribe(&state, move |this, _, event: &SelectEvent, cx| {
                    let SelectEvent::Change(Some(value)) = event else {
                        return;
                    };
                    let choice = ActionChoice {
                        selected: Some(value.clone()),
                        ..choice.clone()
                    };
                    on_choice(this, choice, cx);
                })
                .detach();
                self.states.insert(key, (values, state));
            }
        }
        self.states.retain(|key, _| seen.contains(key));
    }
}

/// An attachment's buttons and menus, for the bottom of its card.
pub fn actions(
    element_id: &ElementId,
    post_id: &str,
    actions: &[ActionCard],
    menus: &Menus,
    on_action: &ActionHandler,
    cx: &App,
) -> Vec<AnyElement> {
    let theme = UiTheme::read(cx).clone();
    let post_id: SharedString = post_id.to_owned().into();
    let controls = actions.iter().filter_map(|action| {
        let id = ElementId::Name(format!("{element_id}.action.{}", action.id).into());
        let off = action.disabled || action.pending;
        match &action.menu {
            Some(menu) => {
                let state = menus.get(&post_id, &action.id)?;
                let select = match menu.source {
                    MenuSource::Static => Select::new(state),
                    // Long lists: type to find.
                    MenuSource::Users | MenuSource::Channels => {
                        crate::ui::combobox::combobox(state)
                    }
                };
                let placeholder = if action.pending {
                    SharedString::from("Sending…")
                } else if action.name.is_empty() {
                    SharedString::from("Choose an option")
                } else {
                    action.name.clone()
                };
                Some(
                    div()
                        .w(theme.space(56.))
                        .child(
                            select
                                .aria_label(if action.name.is_empty() {
                                    SharedString::from("Choose an option")
                                } else {
                                    action.name.clone()
                                })
                                .placeholder(placeholder)
                                .disabled(off),
                        )
                        .into_any_element(),
                )
            }
            None => {
                let choice = ActionChoice {
                    post_id: post_id.clone(),
                    action_id: action.id.clone(),
                    selected: None,
                    cookie: action.cookie.clone(),
                };
                let on_action = on_action.clone();
                let name: SharedString = if action.pending {
                    format!("{} (sending)", action.name).into()
                } else {
                    action.name.clone()
                };
                let mut button = Button::new(id.clone())
                    .variant(action.variant())
                    .size(ButtonSize::Sm);
                if let Some(tint) = action.tint(&theme) {
                    button = button.tint(tint);
                }
                let button = button
                    .aria_label(name)
                    .disabled(off)
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        on_action(choice.clone(), window, cx)
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space(1.5))
                            .when(action.pending, |label| {
                                label.child(Spinner::new(ElementId::Name(
                                    format!("{element_id}.action.{}.spinner", action.id).into(),
                                )))
                            })
                            .child(action.name.clone()),
                    );
                let selector = format!("post-action-{}", action.id);
                Some(
                    div()
                        .debug_selector(move || selector.clone())
                        .child(button)
                        .into_any_element(),
                )
            }
        }
    });
    controls.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn interactive_post() -> ApiPost {
        serde_json::from_value(json!({
            "id": "p1",
            "channel_id": "c1",
            "type": "slack_attachment",
            "props": {"attachments": [{
                "text": "Deploy?",
                "actions": [
                    {"id": "go", "type": "button", "name": "Approve", "style": "primary", "cookie": "k1"},
                    {"id": "", "type": "button", "name": "No id"},
                    {"id": "who", "type": "select", "name": "Reviewer", "data_source": "users"},
                    {"id": "where", "type": "select", "name": "Region",
                     "options": [{"text": "EU", "value": "eu"}], "default_option": "eu"}
                ]
            }]}
        }))
        .unwrap()
    }

    #[test]
    fn actions_come_from_the_attachment() {
        let embeds = crate::embeds::from_post(&interactive_post());
        let actions: Vec<_> = actions_of(&embeds).collect();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_ref()).collect();
        // An action without an id cannot be run, so it is not shown.
        assert_eq!(ids, ["go", "who", "where"]);
        assert_eq!(actions[0].variant(), ButtonVariant::Default);
        assert_eq!(actions[0].cookie.as_ref(), "k1");
        assert!(actions[0].menu.is_none());
        assert_eq!(actions[1].menu.as_ref().unwrap().source, MenuSource::Users);
        let region = actions[2].menu.as_ref().unwrap();
        assert_eq!(region.source, MenuSource::Static);
        assert_eq!(region.options, [("eu".into(), "EU".into())]);
        assert_eq!(region.default.as_ref(), "eu");
    }

    #[test]
    fn integrations_colour_their_buttons() {
        let theme = UiTheme::default();
        let action = |style: &str| ActionCard {
            id: "a".into(),
            name: "Go".into(),
            style: style.into(),
            disabled: false,
            cookie: "".into(),
            menu: None,
            pending: false,
        };
        let tint = |style: &str| action(style).tint(&theme);
        assert_eq!(tint("#3f51b5"), gpui_kit::Rgba::try_from("#3f51b5").ok());
        assert_eq!(tint("good"), Some(theme.colors.success));
        assert_eq!(tint("Warning"), Some(theme.colors.warning));
        // Variants, and a colour that does not parse.
        assert_eq!(tint("primary"), None);
        assert_eq!(tint("danger"), None);
        assert_eq!(tint("#zz"), None);
        assert_eq!(action("danger").variant(), ButtonVariant::Destructive);
    }

    /// `post` as the channel view draws it.
    fn listed(post: &ApiPost) -> crate::channel::Post {
        let keys = matty_notify::MentionKeys::username("me");
        let saved = HashSet::new();
        let zone = jiff::tz::TimeZone::UTC;
        let viewer = crate::transcript::Viewer {
            user_id: "me",
            keys: &keys,
            saved: &saved,
            zone: &zone,
            now: jiff::Timestamp::now(),
            link_previews: true,
            collapsed_threads: false,
            names: crate::settings::NameFormat::default(),
        };
        crate::transcript::listed_post(post, &HashMap::new(), &viewer)
    }

    #[test]
    fn fill_marks_pending_actions_and_lists_people() {
        let mut post = listed(&interactive_post());
        let pending = HashSet::from([("p1".to_owned(), "go".to_owned())]);
        let people = vec![("u1".into(), "Ada".into())];
        fill(std::slice::from_mut(&mut post), &pending, &people, &[]);
        let actions: Vec<_> = actions_of(&post.embeds).collect();
        assert!(actions[0].pending);
        assert!(!actions[2].pending);
        assert_eq!(actions[1].menu.as_ref().unwrap().options, people);
        // A static menu keeps its own options.
        assert_eq!(actions[2].menu.as_ref().unwrap().options.len(), 1);
    }

    fn ephemeral(id: &str, channel: &str, at: i64) -> ApiPost {
        ApiPost {
            id: id.into(),
            channel_id: channel.into(),
            create_at: at,
            kind: EPHEMERAL.into(),
            ..ApiPost::default()
        }
    }

    #[test]
    fn ephemeral_messages_join_their_channel_by_time() {
        let posts = [
            ApiPost {
                id: "a".into(),
                channel_id: "c1".into(),
                create_at: 10,
                ..ApiPost::default()
            },
            ApiPost {
                id: "b".into(),
                channel_id: "c1".into(),
                create_at: 30,
                ..ApiPost::default()
            },
        ];
        let kept = [ephemeral("e1", "c1", 20), ephemeral("e2", "c2", 25)];
        let merged = with_ephemeral(Some(&posts), &kept, "c1").unwrap();
        let ids: Vec<&str> = merged.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "e1", "b"]);
        assert!(with_ephemeral(Some(&posts), &kept, "c3").is_none());
        assert!(with_ephemeral(None, &kept, "c1").is_none(), "still loading");
    }

    /// With collapsed reply threads on, an ephemeral reply shows in its
    /// thread, by time, and in no other.
    #[test]
    fn ephemeral_replies_join_their_thread() {
        let post = |id: &str, root: &str, at: i64| ApiPost {
            id: id.into(),
            channel_id: "c1".into(),
            root_id: root.into(),
            create_at: at,
            ..ApiPost::default()
        };
        let thread: std::sync::Arc<[ApiPost]> = vec![
            post("root", "", 1),
            post("r1", "root", 10),
            post("r2", "root", 30),
        ]
        .into();
        let reply = ApiPost {
            kind: EPHEMERAL.into(),
            ..post("e1", "root", 20)
        };
        let elsewhere = ApiPost {
            kind: EPHEMERAL.into(),
            ..post("e2", "other", 25)
        };
        let merged = with_thread_ephemeral(&thread, &[reply, elsewhere.clone()]);
        let ids: Vec<&str> = merged.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["root", "r1", "e1", "r2"]);
        let alone = with_thread_ephemeral(&thread, &[elsewhere]);
        assert!(std::sync::Arc::ptr_eq(&alone, &thread), "nothing to add");
    }

    #[test]
    fn ephemeral_messages_are_replaced_and_removed() {
        let mut kept = Vec::new();
        assert!(take_ephemeral(&mut kept, &ephemeral("e1", "c1", 20)));
        let mut changed = ephemeral("e1", "c1", 20);
        changed.message = "Done".into();
        assert!(take_ephemeral(&mut kept, &changed));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].message, "Done");
        let mut gone = ephemeral("e1", "c1", 20);
        gone.delete_at = 99;
        assert!(take_ephemeral(&mut kept, &gone));
        assert!(kept.is_empty());
        assert!(!take_ephemeral(&mut kept, &gone), "nothing left to remove");
    }

    struct Probe {
        embeds: Vec<Embed>,
        menus: Menus,
        chosen: Rc<std::cell::RefCell<Vec<ActionChoice>>>,
    }

    impl gpui_kit::Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut post = listed(&interactive_post());
            post.embeds = self.embeds.clone();
            let posts = [post];
            self.menus.sync(&posts, window, cx, |_, _, _| {});
            let chosen = self.chosen.clone();
            div()
                .w(gpui_kit::px(800.))
                .child(crate::embeds::post_embeds(
                    "p1",
                    &self.embeds,
                    false,
                    Rc::new(|_, _, _| {}),
                    Rc::new(|_, _, _| {}),
                    Rc::new(|_, _, _, _| {}),
                    (
                        &self.menus,
                        Rc::new(move |choice, _, _| chosen.borrow_mut().push(choice)),
                    ),
                    cx,
                ))
        }
    }

    /// A press sends the button's choice with its cookie; a button that is
    /// turned off, or already waiting for the server, takes no press.
    #[gpui_kit::test]
    fn buttons_send_their_choice_unless_off_or_waiting(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::ui::theme::init(cx);
            crate::settings::SettingsStore::init(None, cx);
        });
        let mut embeds = crate::embeds::from_post(&interactive_post());
        let Embed::Attachment(card) = &mut embeds[0] else {
            panic!("an attachment");
        };
        card.actions.push(ActionCard {
            id: "off".into(),
            disabled: true,
            ..card.actions[0].clone()
        });
        card.actions.push(ActionCard {
            id: "busy".into(),
            pending: true,
            ..card.actions[0].clone()
        });
        let chosen = Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = cx.add_window({
            let chosen = chosen.clone();
            move |_, _| Probe {
                embeds,
                menus: Menus::default(),
                chosen,
            }
        });
        let mut cx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for id in ["off", "busy", "go"] {
            let bounds = cx
                .debug_bounds(Box::leak(format!("post-action-{id}").into_boxed_str()))
                .unwrap_or_else(|| panic!("{id} drawn"));
            cx.simulate_click(bounds.center(), Default::default());
        }
        assert_eq!(
            *chosen.borrow(),
            [ActionChoice {
                post_id: "p1".into(),
                action_id: "go".into(),
                selected: None,
                cookie: "k1".into(),
            }]
        );
    }

    #[test]
    fn the_session_tracks_answers_and_refusals() {
        use matty_sync::{ChangeSet, CommandError};
        let mut session = Session::default();
        let choice = |action: &str| ActionChoice {
            post_id: "p1".into(),
            action_id: action.into(),
            selected: None,
            cookie: "".into(),
        };
        session.sent(&choice("go"));
        session.sent(&choice("where"));
        let done = session.apply(
            &ChangeSet::PostActionDone {
                post_id: "p1".into(),
                action_id: "go".into(),
            },
            None,
        );
        assert!(done.redraw);
        assert_eq!(session.pending().len(), 1);
        let refused = session.apply(
            &ChangeSet::Failed(CommandError::Server {
                action: "run action",
                target: "p1".into(),
                error: "Action integration error.".into(),
            }),
            None,
        );
        assert!(refused.redraw);
        assert_eq!(
            refused.refused.as_deref(),
            Some("Action integration error.")
        );
        assert!(session.pending().is_empty());
        // Only an ephemeral message in the open channel redraws it.
        let elsewhere = session.apply(
            &ChangeSet::Ephemeral(std::sync::Arc::new(ephemeral("e1", "c2", 5))),
            Some("c1"),
        );
        assert!(!elsewhere.redraw);
        assert_eq!(session.ephemeral().len(), 1);
        assert!(session.dismiss("e1"));
        assert!(session.ephemeral().is_empty());
    }

    /// An ephemeral message reads as a message, from its bot or from the
    /// system, marked as only the viewer's, and never folds under the post
    /// before it.
    #[test]
    fn an_ephemeral_message_shows_as_only_the_viewers() {
        let bot = User {
            id: "bot".into(),
            username: "deploy-bot".into(),
            ..User::default()
        };
        let users = HashMap::from([("bot".to_owned(), bot)]);
        let mut from_bot = ephemeral("e1", "c1", 2_000);
        from_bot.user_id = "bot".into();
        from_bot.message = "Approved.".into();
        let mut from_server = ephemeral("e2", "c1", 3_000);
        from_server.user_id = "me".into();
        from_server.message = "Command not found.".into();
        let mut before = ephemeral("p0", "c1", 1_000);
        before.kind = String::new();
        before.user_id = "bot".into();
        let posts = [before, from_bot, from_server];
        let keys = matty_notify::MentionKeys::username("me");
        let saved = HashSet::new();
        let zone = jiff::tz::TimeZone::UTC;
        let viewer = crate::transcript::Viewer {
            user_id: "me",
            keys: &keys,
            saved: &saved,
            zone: &zone,
            now: jiff::Timestamp::now(),
            link_previews: true,
            collapsed_threads: false,
            names: crate::settings::NameFormat::default(),
        };
        let row =
            crate::sidebar::ChannelRow::new("c1", "town", crate::sidebar::ChannelKind::Public);
        let model =
            crate::transcript::channel_model(&row, Some(&posts), &users, &viewer, None, &[]);
        let shown: Vec<_> = model
            .posts
            .iter()
            .map(|post| (post.author.as_ref(), post.status, post.continues, post.mine))
            .collect();
        use crate::channel::PostStatus::{Ephemeral, Sent};
        assert_eq!(
            shown,
            [
                ("deploy-bot", Sent, false, false),
                ("deploy-bot", Ephemeral, false, false),
                ("System", Ephemeral, false, false),
            ]
        );
        assert_eq!(model.posts[1].kind, crate::channel::PostKind::Regular);
        assert!(
            model.posts[2].author_id.is_empty(),
            "no picture for the system"
        );
    }
}
