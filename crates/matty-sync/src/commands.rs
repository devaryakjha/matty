//! Server calls the views' commands make. Each runs on its own task and
//! answers with a [`Loaded`] that the engine writes like any other result.

use matty_api::models::{Draft, Reaction};
use matty_api::{ApiError, Client, NewChannel, PostPatch};
use tokio::sync::mpsc::UnboundedSender;

use crate::load::Loaded;

/// Page size for the browse and people lists.
const PAGE: u32 = 200;
/// Browse lists at most this many pages of channels, people this many of
/// users; search covers the rest.
const BROWSE_PAGES: u32 = 5;
const PEOPLE_PAGES: u32 = 5;

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
    Create(Box<NewChannel>),
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
            Self::OpenDirect { .. } => "open direct message",
            Self::Join { .. } => "join channel",
            Self::Leave { .. } => "leave channel",
            Self::Browse { .. } => "browse channels",
            Self::People { .. } => "find people",
            Self::Create(_) => "create channel",
        }
    }

    /// The post, channel or user it was about.
    fn target(&self, user_id: &str) -> String {
        match self {
            Self::Edit { post_id, .. }
            | Self::Delete { post_id }
            | Self::Unread { post_id, .. } => post_id.clone(),
            Self::React { reaction, .. } => reaction.post_id.clone(),
            Self::View { channel_id, .. } | Self::DeleteDraft { channel_id, .. } => {
                channel_id.clone()
            }
            Self::SaveDraft(draft) => draft.channel_id.clone(),
            Self::Status { .. } => user_id.to_owned(),
            Self::OpenDirect { user_ids } => user_ids.join(","),
            Self::Join { channel_id } | Self::Leave { channel_id } => channel_id.clone(),
            Self::Browse { team_id } | Self::People { team_id } => team_id.clone(),
            Self::Create(channel) => channel.name.clone(),
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
            Write::OpenDirect { user_ids } => {
                let channel = match user_ids.as_slice() {
                    [other] => client.direct_channel(&user_id, other).await?,
                    others => {
                        let mut everyone = vec![user_id.clone()];
                        everyone.extend(others.iter().filter(|id| **id != user_id).cloned());
                        client.group_channel(&everyone).await?
                    }
                };
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
