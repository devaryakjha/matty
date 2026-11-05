//! Server calls the views' commands make. Each runs on its own task and
//! answers with a [`Loaded`] that the engine writes like any other result.

use matty_api::models::{Draft, Reaction};
use std::collections::HashMap;

use matty_api::{ApiError, ChannelPatch, Client, NewChannel, PostPatch, SearchQuery};
use tokio::sync::mpsc::UnboundedSender;

use crate::load::Loaded;

/// Page size for the browse and people lists.
const PAGE: u32 = 200;
/// Browse lists at most this many pages of channels, people this many of
/// users; search covers the rest.
const BROWSE_PAGES: u32 = 5;
const PEOPLE_PAGES: u32 = 5;
/// Saved posts and recent mentions show this many, newest first.
const LISTING_PAGE: u32 = 60;

/// A server write a command asked for.
#[derive(Debug)]
pub(crate) enum Write {
    Edit {
        post_id: String,
        message: String,
    },
    Delete {
        post_id: String,
    },
    View {
        channel_id: String,
        prev_channel_id: String,
    },
    Unread {
        post_id: String,
        collapsed_threads: bool,
    },
    /// Adds or removes the user's reaction; the store already shows the
    /// change, so a failure undoes it.
    React {
        reaction: Reaction,
        add: bool,
    },
    SaveDraft(Box<Draft>),
    DeleteDraft {
        channel_id: String,
        root_id: String,
    },
    Status {
        status: String,
        dnd_end_time: Option<i64>,
    },
    NotifyProps(std::collections::HashMap<String, String>),
    Profile(matty_api::ProfilePatch),
    /// A new profile picture, or `None` to remove it.
    Picture(Option<(Vec<u8>, String)>),
    /// A direct message with one user, or a group message with several.
    OpenDirect {
        user_ids: Vec<String>,
    },
    Join {
        channel_id: String,
    },
    Leave {
        channel_id: String,
    },
    Archive {
        channel_id: String,
    },
    RemoveMember {
        channel_id: String,
        user_id: String,
    },
    Privacy {
        channel_id: String,
        private: bool,
    },
    /// Reads, not writes, but likewise asked for by a view and answered
    /// once.
    Browse {
        team_id: String,
    },
    People {
        team_id: String,
    },
    Search {
        team_id: String,
        query: Box<SearchQuery>,
    },
    Listing(crate::Listing),
    Reminder {
        post_id: String,
        target_time: i64,
    },
    /// A slash command's suggestions (#210).
    SuggestCommands {
        team_id: String,
        channel_id: String,
        root_id: String,
        user_input: String,
    },
    /// Runs a slash command (#210).
    ExecuteCommand(matty_api::models::ExecuteCommand),
    /// An interactive dialog's values or cancellation (#211); the user id
    /// is filled in as it goes.
    SubmitDialog(Box<matty_api::models::SubmitDialogRequest>),
    /// An interactive message's button press or menu choice (#211).
    PostAction {
        post_id: String,
        action_id: String,
        selected_option: Option<String>,
        cookie: String,
    },
    Pin {
        post_id: String,
        pinned: bool,
    },
    DeletePreferences(Vec<matty_api::models::Preference>),
    Create(Box<NewChannel>),
    Patch {
        channel_id: String,
        patch: ChannelPatch,
    },
    Notify {
        channel_id: String,
        props: HashMap<String, String>,
    },
    AddMembers {
        channel_id: String,
        user_ids: Vec<String>,
    },
    /// Saves preferences already applied to the store.
    Preferences(Vec<matty_api::models::Preference>),
    /// Marks a thread read; the store already shows it read.
    ReadThread {
        team_id: String,
        thread_id: String,
        at: i64,
    },
    /// Marks every followed thread in a team read.
    ReadAllThreads {
        team_id: String,
    },
    /// A new custom sidebar category (#208).
    CreateCategory {
        team_id: String,
        category: Box<matty_api::models::SidebarCategory>,
    },
    /// Saves a renamed category, or one a channel moved into.
    UpdateCategory {
        team_id: String,
        category: Box<matty_api::models::SidebarCategory>,
    },
    /// A team's categories in a new order.
    CategoryOrder {
        team_id: String,
        order: Vec<String>,
    },
    DeleteCategory {
        team_id: String,
        category_id: String,
    },
    /// Follows or unfollows a thread; the store already shows it.
    Follow {
        team_id: String,
        thread_id: String,
        follow: bool,
    },
    /// Finds what a link names, which the cache did not know.
    Resolve(crate::LinkQuery),
    /// Finds who matches a typed `@name`.
    Suggest {
        term: String,
        team_id: Option<String>,
        channel_id: Option<String>,
    },
}

impl Write {
    /// What the user asked, for [`CommandError::Server`](crate::CommandError::Server).
    pub(crate) fn action(&self) -> &'static str {
        match self {
            Self::Edit { .. } => "edit",
            Self::Delete { .. } => "delete",
            Self::View { .. } => "view",
            Self::Unread { .. } => "mark unread",
            Self::React { add: true, .. } => "add reaction",
            Self::React { add: false, .. } => "remove reaction",
            Self::SaveDraft(_) => "save draft",
            Self::DeleteDraft { .. } => "delete draft",
            Self::Status { .. } => "set status",
            Self::NotifyProps(_) => "save notification preferences",
            Self::Profile(_) => "save your profile",
            Self::Picture(Some(_)) => "change your picture",
            Self::Picture(None) => "remove your picture",
            Self::OpenDirect { .. } => "open direct message",
            Self::Join { .. } => "join channel",
            Self::Leave { .. } => "leave channel",
            Self::Archive { .. } => "archive channel",
            Self::RemoveMember { .. } => "remove member",
            Self::Privacy { .. } => "change channel privacy",
            Self::Browse { .. } => "browse channels",
            Self::People { .. } => "find people",
            Self::Search { .. } => "search",
            Self::Listing(crate::Listing::Saved) => "load saved messages",
            Self::Listing(crate::Listing::Pinned { .. }) => "load pinned messages",
            Self::Listing(crate::Listing::Mentions { .. }) => "load recent mentions",
            Self::Pin { pinned: true, .. } => "pin message",
            Self::Pin { pinned: false, .. } => "unpin message",
            Self::DeletePreferences(_) => "delete preferences",
            Self::Reminder { .. } => "set reminder",
            Self::PostAction { .. } => "run action",
            Self::SubmitDialog(_) => "submit dialog",
            Self::SuggestCommands { .. } => "suggest commands",
            Self::ExecuteCommand(_) => "run command",
            Self::Create(_) => "create channel",
            Self::Patch { .. } => "change channel",
            Self::Notify { .. } => "change notifications",
            Self::AddMembers { .. } => "add members",
            Self::Preferences(_) => "save preferences",
            Self::ReadThread { .. } => "mark thread read",
            Self::ReadAllThreads { .. } => "mark threads read",
            Self::CreateCategory { .. } => "create category",
            Self::UpdateCategory { .. } => "change category",
            Self::CategoryOrder { .. } => "reorder categories",
            Self::DeleteCategory { .. } => "delete category",
            Self::Follow { follow: true, .. } => "follow thread",
            Self::Follow { follow: false, .. } => "unfollow thread",
            Self::Resolve(_) => "open link",
            Self::Suggest { .. } => "suggest people",
        }
    }

    /// The post, channel or user it was about.
    fn target(&self, user_id: &str) -> String {
        match self {
            Self::Edit { post_id, .. }
            | Self::Pin { post_id, .. }
            | Self::Reminder { post_id, .. }
            | Self::PostAction { post_id, .. }
            | Self::Delete { post_id }
            | Self::Unread { post_id, .. } => post_id.clone(),
            Self::React { reaction, .. } => reaction.post_id.clone(),
            Self::View { channel_id, .. } | Self::DeleteDraft { channel_id, .. } => {
                channel_id.clone()
            }
            Self::SaveDraft(draft) => draft.channel_id.clone(),
            Self::Status { .. } | Self::NotifyProps(_) | Self::Profile(_) | Self::Picture(_) => {
                user_id.to_owned()
            }
            Self::OpenDirect { user_ids } => user_ids.join(","),
            Self::Join { channel_id }
            | Self::Leave { channel_id }
            | Self::Archive { channel_id }
            | Self::RemoveMember { channel_id, .. }
            | Self::Privacy { channel_id, .. }
            | Self::Patch { channel_id, .. }
            | Self::Notify { channel_id, .. }
            | Self::AddMembers { channel_id, .. } => channel_id.clone(),
            Self::Listing(crate::Listing::Saved) => user_id.to_owned(),
            Self::Listing(crate::Listing::Pinned { channel_id }) => channel_id.clone(),
            Self::Listing(crate::Listing::Mentions { team_id, .. })
            | Self::Browse { team_id }
            | Self::People { team_id }
            | Self::Search { team_id, .. }
            | Self::ReadAllThreads { team_id } => team_id.clone(),
            Self::CreateCategory { category, .. } | Self::UpdateCategory { category, .. } => {
                category.display_name.clone()
            }
            Self::DeleteCategory { category_id, .. } => category_id.clone(),
            Self::CategoryOrder { team_id, .. } => team_id.clone(),
            Self::ReadThread { thread_id, .. } | Self::Follow { thread_id, .. } => {
                thread_id.clone()
            }
            Self::Create(channel) => channel.name.clone(),
            Self::Suggest { term, .. } => term.clone(),
            Self::SubmitDialog(submit) => submit.callback_id.clone(),
            Self::SuggestCommands { channel_id, .. } => channel_id.clone(),
            Self::ExecuteCommand(command) => command.channel_id.clone(),
            Self::Resolve(query) => match query {
                crate::LinkQuery::Post { post_id } => post_id.clone(),
                crate::LinkQuery::Channel { name, .. } => name.clone(),
                crate::LinkQuery::User { username } => username.clone(),
            },
            Self::Preferences(preferences) | Self::DeletePreferences(preferences) => preferences
                .iter()
                .map(|preference| preference.name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// The server call that keeps a draft in sync: none unless the server allows
/// synced drafts, and a delete for an empty one.
pub(crate) fn draft_write(synced: bool, draft: Draft) -> Option<Write> {
    if !synced {
        return None;
    }
    Some(if draft.message.is_empty() && draft.file_ids.is_empty() {
        Write::DeleteDraft {
            channel_id: draft.channel_id,
            root_id: draft.root_id,
        }
    } else {
        Write::SaveDraft(Box::new(draft))
    })
}

/// Makes the call and sends its result back.
pub(crate) async fn run(
    client: Client,
    user_id: String,
    write: Write,
    results: UnboundedSender<Loaded>,
) {
    // A dialog showing an App's form goes back to the App (#226).
    let write = match write {
        Write::SubmitDialog(submit) if crate::apps::is_app_dialog(&submit.url) => {
            return crate::apps::submit(client, *submit, results).await;
        }
        other => other,
    };
    let action = write.action();
    let target = write.target(&user_id);
    let outcome: Result<Option<Loaded>, ApiError> = async {
        Ok(match write {
            Write::Edit { post_id, message } => {
                let patch = PostPatch {
                    message: Some(message),
                    ..PostPatch::default()
                };
                Some(Loaded::Edited(Box::new(
                    client.patch_post(&post_id, &patch).await?,
                )))
            }
            Write::ReadThread {
                team_id,
                thread_id,
                at,
            } => {
                let thread = client
                    .mark_thread_read(&user_id, &team_id, &thread_id, at)
                    .await?;
                Some(Loaded::Threads {
                    team_id,
                    threads: vec![thread],
                    total: None,
                })
            }
            Write::ReadAllThreads { team_id } => {
                client.mark_all_threads_read(&user_id, &team_id).await?;
                None
            }
            Write::Resolve(query) => {
                let answer = match resolve(&client, &query).await {
                    Ok(answer) => Some(answer),
                    // Gone, or not the user's to see: the app says so.
                    Err(ApiError::Server {
                        status: 403 | 404, ..
                    }) => None,
                    Err(error) => return Err(error),
                };
                Some(Loaded::Resolved { query, answer })
            }
            Write::Suggest {
                term,
                team_id,
                channel_id,
            } => {
                // Suggestions are a nicety: a failure leaves the local ones
                // rather than an error the user did not cause.
                let users = match client
                    .autocomplete_users(&term, team_id.as_deref(), channel_id.as_deref())
                    .await
                {
                    Ok(found) => found
                        .users
                        .into_iter()
                        .chain(found.out_of_channel)
                        .collect(),
                    Err(error) => {
                        tracing::warn!(%error, "could not suggest people");
                        Vec::new()
                    }
                };
                Some(Loaded::Suggested { term, users })
            }
            Write::Follow {
                team_id,
                thread_id,
                follow,
            } => {
                if follow {
                    client.follow_thread(&user_id, &team_id, &thread_id).await?;
                } else {
                    client
                        .unfollow_thread(&user_id, &team_id, &thread_id)
                        .await?;
                }
                None
            }
            Write::Delete { post_id } => {
                client.delete_post(&post_id).await?;
                Some(Loaded::Deleted { post_id })
            }
            Write::View {
                channel_id,
                prev_channel_id,
            } => {
                client.view_channel(&channel_id, &prev_channel_id).await?;
                None
            }
            Write::Unread {
                post_id,
                collapsed_threads,
            } => {
                let unread = client
                    .set_post_unread(&user_id, &post_id, collapsed_threads)
                    .await?;
                Some(Loaded::Unread(Box::new(unread)))
            }
            Write::React { reaction, add } => {
                let result = if add {
                    client
                        .add_reaction(&reaction.user_id, &reaction.post_id, &reaction.emoji_name)
                        .await
                        .map(|_| ())
                } else {
                    client
                        .remove_reaction(&reaction.user_id, &reaction.post_id, &reaction.emoji_name)
                        .await
                };
                if let Err(error) = result {
                    Some(Loaded::ReactFailed {
                        reaction: Box::new(reaction),
                        added: add,
                        error: told(&error),
                    })
                } else {
                    None
                }
            }
            Write::SaveDraft(draft) => {
                client.save_draft(&draft).await?;
                None
            }
            Write::DeleteDraft {
                channel_id,
                root_id,
            } => {
                client.delete_draft(&user_id, &channel_id, &root_id).await?;
                None
            }
            Write::Status {
                status,
                dnd_end_time,
            } => {
                let status = client.set_status(&user_id, &status, dnd_end_time).await?;
                Some(Loaded::Statuses(vec![status]))
            }
            Write::NotifyProps(notify_props) => {
                let user = client.patch_notify_props(&user_id, &notify_props).await?;
                Some(Loaded::Users(vec![user]))
            }
            Write::Profile(patch) => {
                let user = client.patch_profile(&user_id, &patch).await?;
                Some(Loaded::Users(vec![user]))
            }
            Write::Picture(picture) => {
                match picture {
                    Some((bytes, file_name)) => {
                        client.set_profile_image(&user_id, bytes, file_name).await?
                    }
                    None => client.reset_profile_image(&user_id).await?,
                }
                // The new `last_picture_update` refetches the picture.
                Some(Loaded::Users(vec![client.me().await?]))
            }
            Write::OpenDirect { user_ids } => {
                let channel = match user_ids.as_slice() {
                    [other] => client.direct_channel(&user_id, other).await?,
                    others => {
                        let mut everyone = vec![user_id.clone()];
                        everyone.extend(others.iter().filter(|id| **id != user_id).cloned());
                        client.group_channel(&everyone).await?
                    }
                };
                // A conversation the user closed shows again.
                if let Some((category, name)) = matty_store::sidebar::show_preference(
                    &channel.kind,
                    &channel.id,
                    &channel.name,
                    &user_id,
                ) {
                    client
                        .save_preferences(
                            &user_id,
                            &[matty_api::models::Preference {
                                user_id: user_id.clone(),
                                category: category.to_owned(),
                                name,
                                value: "true".to_owned(),
                            }],
                        )
                        .await?;
                }
                let member = client.my_channel_member(&channel.id).await?;
                Some(Loaded::Ready {
                    channel: Box::new(channel),
                    member: Box::new(member),
                })
            }
            Write::Join { channel_id } => {
                let member = client.add_channel_member(&channel_id, &user_id).await?;
                let channel = client.channel(&channel_id).await?;
                Some(Loaded::Ready {
                    channel: Box::new(channel),
                    member: Box::new(member),
                })
            }
            Write::Leave { channel_id } => {
                client.remove_channel_member(&channel_id, &user_id).await?;
                Some(Loaded::Left { channel_id })
            }
            Write::RemoveMember {
                channel_id,
                user_id,
            } => {
                client.remove_channel_member(&channel_id, &user_id).await?;
                Some(Loaded::Stats(client.channel_stats(&channel_id).await?))
            }
            Write::Archive { channel_id } => {
                client.archive_channel(&channel_id).await?;
                Some(Loaded::Left { channel_id })
            }
            Write::Privacy {
                channel_id,
                private,
            } => {
                let channel = client.set_channel_privacy(&channel_id, private).await?;
                Some(Loaded::Channel {
                    channel: Box::new(channel),
                    member: None,
                })
            }
            Write::Browse { team_id } => {
                let mut channels = Vec::new();
                for page in 0..BROWSE_PAGES {
                    let batch = client.public_channels(&team_id, page, PAGE).await?;
                    let last = batch.len() < PAGE as usize;
                    channels.extend(batch);
                    if last {
                        break;
                    }
                }
                let ids: Vec<String> = channels.iter().map(|c| c.id.clone()).collect();
                let members = if ids.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    client.channel_member_counts(&ids).await?
                };
                Some(Loaded::Directory {
                    team_id,
                    channels,
                    members,
                })
            }
            Write::People { team_id } => {
                let mut users = Vec::new();
                for page in 0..PEOPLE_PAGES {
                    let batch = client.team_users(&team_id, page, PAGE).await?;
                    let last = batch.len() < PAGE as usize;
                    users.extend(batch);
                    if last {
                        break;
                    }
                }
                Some(Loaded::People { team_id, users })
            }
            Write::Search { team_id, query } => {
                let results = client.search_posts(&team_id, &query).await?;
                let posts: Vec<_> = results.list.ordered().cloned().collect();
                Some(Loaded::Search {
                    more: posts.len() >= query.per_page as usize,
                    posts,
                    terms: query.terms,
                    page: query.page,
                })
            }
            Write::CreateCategory { team_id, category } => {
                client.create_category(&team_id, &category).await?;
                categories(&client, team_id).await?
            }
            Write::UpdateCategory { team_id, category } => {
                client
                    .update_categories(&team_id, std::slice::from_ref(&*category))
                    .await?;
                categories(&client, team_id).await?
            }
            Write::CategoryOrder { team_id, order } => {
                client.set_category_order(&team_id, &order).await?;
                categories(&client, team_id).await?
            }
            Write::DeleteCategory {
                team_id,
                category_id,
            } => {
                client.delete_category(&team_id, &category_id).await?;
                categories(&client, team_id).await?
            }
            Write::Patch { channel_id, patch } => {
                let channel = client.patch_channel(&channel_id, &patch).await?;
                Some(Loaded::Channel {
                    channel: Box::new(channel),
                    member: None,
                })
            }
            Write::Notify { channel_id, props } => {
                client
                    .update_channel_notify_props(&channel_id, &user_id, &props)
                    .await?;
                let member = client.my_channel_member(&channel_id).await?;
                let channel = client.channel(&channel_id).await?;
                Some(Loaded::Channel {
                    channel: Box::new(channel),
                    member: Some(Box::new(member)),
                })
            }
            Write::Preferences(preferences) => {
                client.save_preferences(&user_id, &preferences).await?;
                None
            }
            Write::DeletePreferences(preferences) => {
                client.delete_preferences(&user_id, &preferences).await?;
                None
            }
            Write::Reminder {
                post_id,
                target_time,
            } => {
                client
                    .set_post_reminder(&user_id, &post_id, target_time)
                    .await?;
                None
            }
            Write::PostAction {
                post_id,
                action_id,
                selected_option,
                cookie,
            } => {
                client
                    .do_post_action(&post_id, &action_id, selected_option.as_deref(), &cookie)
                    .await?;
                Some(Loaded::PostActionDone { post_id, action_id })
            }
            Write::SuggestCommands {
                team_id,
                channel_id,
                root_id,
                user_input,
            } => {
                let suggestions =
                    suggest_commands(&client, &team_id, &channel_id, &root_id, &user_input).await;
                Some(Loaded::CommandSuggestions {
                    channel_id,
                    root_id,
                    user_input,
                    suggestions,
                })
            }
            Write::ExecuteCommand(command) => {
                let outcome = match client.execute_command(&command).await {
                    Ok(mut response) => {
                        response.goto_location =
                            on_server(client.base_url().as_str(), &response.goto_location);
                        Ok(response)
                    }
                    // Unknown, refused or broken: the server's words show
                    // where it was typed. Offline is the usual toast.
                    Err(ApiError::Server { message, .. }) => Err(message),
                    Err(error) => return Err(error),
                };
                Some(Loaded::CommandRan { command, outcome })
            }
            Write::SubmitDialog(mut submit) => {
                submit.user_id.clone_from(&user_id);
                let answer = client.submit_dialog(&submit).await?;
                Some(Loaded::DialogSubmitted {
                    callback_id: submit.callback_id,
                    cancelled: submit.cancelled,
                    answer,
                })
            }
            Write::Pin { post_id, pinned } => {
                if pinned {
                    client.pin_post(&post_id).await?;
                } else {
                    client.unpin_post(&post_id).await?;
                }
                None
            }
            Write::Listing(listing) => {
                let list = match &listing {
                    crate::Listing::Saved => client.saved_posts(&user_id, 0, LISTING_PAGE).await,
                    crate::Listing::Pinned { channel_id } => client.pinned_posts(channel_id).await,
                    crate::Listing::Mentions { team_id, terms } => {
                        let mut query = SearchQuery::new(terms.clone());
                        query.is_or_search = true;
                        query.per_page = LISTING_PAGE;
                        client
                            .search_posts(team_id, &query)
                            .await
                            .map(|found| found.list)
                    }
                };
                let list = match list {
                    Ok(list) => list,
                    // Unreachable: the cache answers what it can.
                    Err(error) if told(&error).is_none() => {
                        return Ok(Some(Loaded::ListingOffline(listing)));
                    }
                    Err(error) => return Err(error),
                };
                let mut posts: Vec<_> = list.ordered().cloned().collect();
                posts.sort_by_key(|post| std::cmp::Reverse(post.create_at));
                Some(Loaded::Listing { listing, posts })
            }
            Write::AddMembers {
                channel_id,
                user_ids,
            } => {
                for id in &user_ids {
                    client.add_channel_member(&channel_id, id).await?;
                }
                Some(Loaded::Stats(client.channel_stats(&channel_id).await?))
            }
            Write::Create(new) => {
                let channel = client.create_channel(&new).await?;
                let member = client.my_channel_member(&channel.id).await?;
                Some(Loaded::Ready {
                    channel: Box::new(channel),
                    member: Box::new(member),
                })
            }
        })
    }
    .await;
    let message = match outcome {
        Ok(Some(message)) => message,
        Ok(None) => return,
        Err(error) => Loaded::WriteFailed {
            action,
            target,
            error: told(&error),
        },
    };
    let _ = results.send(message);
}

/// What the server said, or `None` when it could not be reached: the
/// network is down or the request timed out, and the app says the user is
/// offline rather than quoting a transport error (#163).
fn told(error: &ApiError) -> Option<String> {
    match error {
        ApiError::Network(_) | ApiError::Timeout => None,
        error => Some(error.to_string()),
    }
}

/// What may come next in a slash command: the server's suggestions, or on
/// a server without them, the team's commands that start with what was
/// typed. Suggestions are a nicety, so a failure is none.
async fn suggest_commands(
    client: &Client,
    team_id: &str,
    channel_id: &str,
    root_id: &str,
    user_input: &str,
) -> Vec<matty_api::models::CommandSuggestion> {
    match client
        .command_suggestions(team_id, user_input, channel_id, root_id)
        .await
    {
        Ok(found) => return found,
        Err(error) => tracing::debug!(%error, "no command suggestions; listing commands"),
    }
    // Only a command name has suggestions this way.
    let Some(typed) = user_input
        .strip_prefix('/')
        .filter(|typed| !typed.contains(char::is_whitespace))
    else {
        return Vec::new();
    };
    match client.autocomplete_commands(team_id).await {
        Ok(commands) => command_names(&commands, typed),
        Err(error) => {
            tracing::warn!(%error, "could not list commands");
            Vec::new()
        }
    }
}

/// The commands whose trigger starts with `typed`, as suggestions.
pub(crate) fn command_names(
    commands: &[matty_api::models::SlashCommand],
    typed: &str,
) -> Vec<matty_api::models::CommandSuggestion> {
    let typed = typed.to_lowercase();
    let mut found: Vec<_> = commands
        .iter()
        .filter(|command| command.auto_complete && command.delete_at == 0)
        .filter(|command| command.trigger.to_lowercase().starts_with(&typed))
        .map(|command| matty_api::models::CommandSuggestion {
            complete: command.trigger.clone(),
            suggestion: command.trigger.clone(),
            hint: command.auto_complete_hint.clone(),
            description: if command.auto_complete_desc.is_empty() {
                command.description.clone()
            } else {
                command.auto_complete_desc.clone()
            },
            icon_data: String::new(),
        })
        .collect();
    found.sort_by(|a, b| a.complete.cmp(&b.complete));
    found.dedup_by(|a, b| a.complete == b.complete);
    found
}

/// `location` as a URL: a path such as `/team/channels/name` is on the
/// server at `base`; anything else stays as it is.
pub(crate) fn on_server(base: &str, location: &str) -> String {
    if !location.starts_with('/') || location.starts_with("//") {
        return location.to_owned();
    }
    let root = base.trim_end_matches('/');
    format!("{root}{location}")
}

/// A team's categories after a change to them, so the sidebar shows the
/// server's result without waiting for the socket.
async fn categories(client: &Client, team_id: String) -> Result<Option<Loaded>, ApiError> {
    let categories = client.sidebar_categories(&team_id).await?;
    Ok(Some(Loaded::Categories {
        team_id,
        categories,
    }))
}

/// Asks the server what a link names.
async fn resolve(client: &Client, query: &crate::LinkQuery) -> Result<crate::LinkAnswer, ApiError> {
    use crate::{LinkAnswer, LinkQuery};
    use std::sync::Arc;
    Ok(match query {
        LinkQuery::Post { post_id } => {
            let post = client.get_post(post_id).await?;
            LinkAnswer::Channel(Arc::new(client.channel(&post.channel_id).await?))
        }
        LinkQuery::Channel { team_id, name } => {
            LinkAnswer::Channel(Arc::new(client.channel_by_name(team_id, name).await?))
        }
        LinkQuery::User { username } => {
            let users = client
                .users_by_usernames(std::slice::from_ref(username))
                .await?;
            let user = users.into_iter().next().ok_or(ApiError::Server {
                status: 404,
                id: "store.sql_user.missing".into(),
                message: "no such user".into(),
                request_id: None,
            })?;
            LinkAnswer::User(Arc::new(user))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(message: &str) -> Draft {
        Draft {
            channel_id: "c1".into(),
            root_id: "r1".into(),
            message: message.into(),
            ..Draft::default()
        }
    }

    #[test]
    fn command_paths_are_on_the_server() {
        let base = "https://chat.example.com/mm/";
        assert_eq!(
            on_server(base, "/team/channels/town-square"),
            "https://chat.example.com/mm/team/channels/town-square"
        );
        assert_eq!(
            on_server(base, "https://example.org/x"),
            "https://example.org/x"
        );
        assert_eq!(on_server(base, ""), "");
    }

    #[test]
    fn listed_commands_suggest_names() {
        let command = |trigger: &str, auto_complete: bool| matty_api::models::SlashCommand {
            trigger: trigger.into(),
            auto_complete,
            auto_complete_desc: format!("{trigger} it"),
            ..Default::default()
        };
        let commands = [
            command("shrug", true),
            command("shortcuts", true),
            command("secret", false),
            command("me", true),
        ];
        let names: Vec<String> = command_names(&commands, "Sh")
            .into_iter()
            .map(|found| found.complete)
            .collect();
        assert_eq!(names, ["shortcuts", "shrug"]);
    }

    #[test]
    fn drafts_stay_local_unless_the_server_syncs_them() {
        assert!(draft_write(false, draft("hi")).is_none());
        assert!(draft_write(false, draft("")).is_none());
        assert!(matches!(
            draft_write(true, draft("hi")),
            Some(Write::SaveDraft(saved)) if saved.message == "hi"
        ));
        assert!(matches!(
            draft_write(true, draft("")),
            Some(Write::DeleteDraft { channel_id, root_id }) if channel_id == "c1" && root_id == "r1"
        ));
        let with_file = Draft {
            file_ids: vec!["f1".into()],
            ..draft("")
        };
        assert!(matches!(
            draft_write(true, with_file),
            Some(Write::SaveDraft(_))
        ));
    }
}
