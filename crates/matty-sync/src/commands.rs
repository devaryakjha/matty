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
    /// Follows or unfollows a thread; the store already shows it.
    Follow {
        team_id: String,
        thread_id: String,
        follow: bool,
    },
    /// Finds what a link names, which the cache did not know.
    Resolve(crate::LinkQuery),
}

impl Write {
    /// What the user asked, for [`CommandError::Server`](crate::CommandError::Server).
    fn action(&self) -> &'static str {
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
            Self::OpenDirect { .. } => "open direct message",
            Self::Join { .. } => "join channel",
            Self::Leave { .. } => "leave channel",
            Self::Browse { .. } => "browse channels",
            Self::People { .. } => "find people",
            Self::Search { .. } => "search",
            Self::Listing(crate::Listing::Saved) => "load saved messages",
            Self::Listing(crate::Listing::Pinned { .. }) => "load pinned messages",
            Self::Listing(crate::Listing::Mentions { .. }) => "load recent mentions",
            Self::Pin { pinned: true, .. } => "pin message",
            Self::Pin { pinned: false, .. } => "unpin message",
            Self::DeletePreferences(_) => "delete preferences",
            Self::Create(_) => "create channel",
            Self::Patch { .. } => "change channel",
            Self::Notify { .. } => "change notifications",
            Self::AddMembers { .. } => "add members",
            Self::Preferences(_) => "save preferences",
            Self::ReadThread { .. } => "mark thread read",
            Self::ReadAllThreads { .. } => "mark threads read",
            Self::Follow { follow: true, .. } => "follow thread",
            Self::Follow { follow: false, .. } => "unfollow thread",
            Self::Resolve(_) => "open link",
        }
    }

    /// The post, channel or user it was about.
    fn target(&self, user_id: &str) -> String {
        match self {
            Self::Edit { post_id, .. }
            | Self::Pin { post_id, .. }
            | Self::Delete { post_id }
            | Self::Unread { post_id, .. } => post_id.clone(),
            Self::React { reaction, .. } => reaction.post_id.clone(),
            Self::View { channel_id, .. } | Self::DeleteDraft { channel_id, .. } => {
                channel_id.clone()
            }
            Self::SaveDraft(draft) => draft.channel_id.clone(),
            Self::Status { .. } | Self::NotifyProps(_) => user_id.to_owned(),
            Self::OpenDirect { user_ids } => user_ids.join(","),
            Self::Join { channel_id }
            | Self::Leave { channel_id }
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
            Self::ReadThread { thread_id, .. } | Self::Follow { thread_id, .. } => {
                thread_id.clone()
            }
            Self::Create(channel) => channel.name.clone(),
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
                        error: error.to_string(),
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
                Some(Loaded::Search {
                    posts: results.list.ordered().cloned().collect(),
                    terms: query.terms,
                })
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
                    crate::Listing::Saved => client.saved_posts(&user_id, 0, LISTING_PAGE).await?,
                    crate::Listing::Pinned { channel_id } => {
                        client.pinned_posts(channel_id).await?
                    }
                    crate::Listing::Mentions { team_id, terms } => {
                        let mut query = SearchQuery::new(terms.clone());
                        query.is_or_search = true;
                        query.per_page = LISTING_PAGE;
                        client.search_posts(team_id, &query).await?.list
                    }
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
            error: error.to_string(),
        },
    };
    let _ = results.send(message);
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
