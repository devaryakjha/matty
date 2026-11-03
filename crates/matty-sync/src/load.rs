//! Initial load (PLAN §6.3): what the engine fetches on connect. Fetching
//! runs in tasks on the sync runtime; every result comes back to the engine
//! loop as a [`Loaded`] message, which writes it in one transaction.

use std::collections::BTreeSet;

use matty_api::models::{
    Channel, ChannelMember, Emoji, PostList, Preference, SidebarCategories, Status, Team, User,
    UserThread,
};
use matty_api::ws::WsSignal;
use matty_api::{ApiError, Client, Fetched, PostsQuery, ServerCapabilities, ThreadsQuery};
use tokio::sync::mpsc::UnboundedSender;

use crate::events::Fetch;

/// Posts fetched on each side of the first unread when a channel opens.
pub(crate) const OPEN_LIMIT: u32 = 60;

/// A result of the load, for the engine to write.
#[derive(Debug)]
pub(crate) enum Loaded {
    /// Profiles fetched for authors the cache did not have.
    Users(Vec<User>),
    /// The server has ended the session, or could not be told.
    SignedOut,
    /// Who the user is and what the server offers.
    Account {
        me: Box<User>,
        capabilities: Box<ServerCapabilities>,
        preferences: Vec<Preference>,
        teams: Vec<Team>,
    },
    /// One team's channels and sidebar, with the people its direct
    /// messages are with.
    Team {
        team_id: String,
        channels: Vec<Channel>,
        members: Vec<ChannelMember>,
        categories: SidebarCategories,
        users: Vec<User>,
    },
    /// A team could not load; the others carry on.
    TeamFailed { team_id: String, error: String },
    /// A channel the user opened or joined, to store and switch to.
    Ready {
        channel: Box<Channel>,
        member: Box<ChannelMember>,
    },
    /// The user left a channel.
    Left { channel_id: String },
    /// The account itself could not load.
    Failed(String),
    /// Custom emoji.
    Emoji(Vec<Emoji>),
    /// Statuses for the people in direct messages.
    Statuses(Vec<Status>),
    /// Every team has been tried.
    Done,
    /// An opened channel's posts around the first unread.
    Posts { channel_id: String, posts: PostList },
    /// An opened channel's posts could not load.
    PostsFailed { channel_id: String, error: String },
    /// A channel an event named, with the user's membership when they have one.
    Channel {
        channel: Box<Channel>,
        member: Option<Box<ChannelMember>>,
    },
    /// A team the user joined; its channels follow as [`Loaded::Team`].
    TeamInfo(Box<Team>),
    /// A team's sidebar categories, reloaded.
    Categories {
        team_id: String,
        categories: SidebarCategories,
    },
    /// The server's config or license changed.
    Capabilities(Box<ServerCapabilities>),
    /// Something from the WebSocket.
    Ws(WsSignal),
    /// What changed in a channel since its cursor, deletes included.
    Since { channel_id: String, posts: PostList },
    /// A channel's latest page, replacing its cached posts after a long gap.
    Reset { channel_id: String, posts: PostList },
    /// Threads that changed in a team since its cursor.
    Threads {
        team_id: String,
        threads: Vec<UserThread>,
    },
    /// A resync finished.
    Resynced,
    /// The server has a queued post.
    Sent {
        pending_post_id: String,
        post: Box<matty_api::models::Post>,
    },
    /// A queued post could not be sent.
    SendFailed {
        pending_post_id: String,
        error: String,
    },
    /// An edit the user made, as the server stored it.
    Edited(Box<matty_api::models::Post>),
    /// A delete the user made went through.
    Deleted { post_id: String },
    /// The read state after marking a post unread.
    Unread(Box<matty_api::models::ChannelUnreadAt>),
    /// A reaction the store already shows did not reach the server.
    ReactFailed {
        reaction: Box<matty_api::models::Reaction>,
        added: bool,
        error: String,
    },
    /// A command's server call failed.
    WriteFailed {
        action: &'static str,
        target: String,
        error: String,
    },
}

/// Runs a fetch an event asked for. Failures are logged: the next resync
/// brings the cache back in line.
pub(crate) async fn fetch(
    client: Client,
    user_id: String,
    what: Fetch,
    results: UnboundedSender<Loaded>,
) {
    let outcome: Result<(), ApiError> = async {
        match &what {
            Fetch::Channel(channel_id) => {
                let channel = client.channel(channel_id).await?;
                // Not a member (a public channel someone else made): the
                // sidebar leaves it out, but posts and names still resolve.
                let member = client.my_channel_member(channel_id).await.ok();
                let _ = results.send(Loaded::Channel {
                    channel: Box::new(channel),
                    member: member.map(Box::new),
                });
            }
            Fetch::Team(team_id) => {
                let teams = client.my_teams().await?;
                if let Some(joined) = teams.into_iter().find(|t| &t.id == team_id) {
                    let _ = results.send(Loaded::TeamInfo(Box::new(joined)));
                    let loaded = team(&client, &user_id, team_id).await?;
                    let _ = results.send(loaded);
                }
            }
            Fetch::Categories(team_id) => {
                let categories = client.sidebar_categories(team_id).await?;
                let _ = results.send(Loaded::Categories {
                    team_id: team_id.clone(),
                    categories,
                });
            }
            Fetch::Capabilities => {
                let capabilities = client.capabilities().await?;
                let _ = results.send(Loaded::Capabilities(Box::new(capabilities)));
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        tracing::warn!(?what, %error, "a fetch for an event failed");
    }
}

/// Loads the account, then every team at once, then emoji and statuses.
pub(crate) async fn run(client: Client, results: UnboundedSender<Loaded>) {
    let account = tokio::try_join!(
        client.me(),
        client.capabilities(),
        client.preferences(),
        client.my_teams(),
    );
    let (me, capabilities, preferences, teams) = match account {
        Ok(account) => account,
        Err(error) => {
            let _ = results.send(Loaded::Failed(error.to_string()));
            return;
        }
    };
    let user_id = me.id.clone();
    let team_ids: Vec<String> = teams
        .iter()
        .filter(|team| team.delete_at == 0)
        .map(|team| team.id.clone())
        .collect();
    let custom_emoji = capabilities.custom_emoji;
    if results
        .send(Loaded::Account {
            me: Box::new(me),
            capabilities: Box::new(capabilities),
            preferences,
            teams,
        })
        .is_err()
    {
        return;
    }

    let mut tasks = tokio::task::JoinSet::new();
    for team_id in team_ids {
        let client = client.clone();
        let user_id = user_id.clone();
        tasks.spawn(async move {
            let loaded = team(&client, &user_id, &team_id).await;
            (team_id, loaded)
        });
    }
    let mut partners = BTreeSet::new();
    while let Some(joined) = tasks.join_next().await {
        let message = match joined {
            Ok((_, Ok(loaded))) => {
                if let Loaded::Team { channels, .. } = &loaded {
                    partners.extend(dm_partners(channels, &user_id));
                }
                loaded
            }
            Ok((team_id, Err(error))) => Loaded::TeamFailed {
                team_id,
                error: error.to_string(),
            },
            Err(error) => {
                tracing::error!(%error, "a team load task failed");
                continue;
            }
        };
        if results.send(message).is_err() {
            return;
        }
    }

    if !partners.is_empty() {
        let ids: Vec<String> = partners.into_iter().collect();
        match client.statuses(&ids).await {
            Ok(statuses) => {
                let _ = results.send(Loaded::Statuses(statuses));
            }
            Err(error) => tracing::warn!(%error, "could not load statuses"),
        }
    }
    if custom_emoji {
        match client.all_emoji().await {
            Ok(emoji) => {
                let _ = results.send(Loaded::Emoji(emoji));
            }
            Err(error) => tracing::warn!(%error, "could not load custom emoji"),
        }
    }
    let _ = results.send(Loaded::Done);
}

async fn team(client: &Client, user_id: &str, team_id: &str) -> Result<Loaded, ApiError> {
    let (channels, members, categories) = tokio::try_join!(
        client.my_channels(team_id),
        client.my_channel_members(user_id, team_id),
        client.sidebar_categories(team_id),
    )?;
    let partners: Vec<String> = dm_partners(&channels, user_id).collect();
    let users = if partners.is_empty() {
        Vec::new()
    } else {
        client.users_by_ids(&partners).await?
    };
    Ok(Loaded::Team {
        team_id: team_id.to_owned(),
        channels,
        members,
        categories,
        users,
    })
}

/// The other people in direct channels (named `a__b`).
fn dm_partners<'a>(channels: &'a [Channel], me: &'a str) -> impl Iterator<Item = String> + 'a {
    channels
        .iter()
        .filter(|channel| channel.kind == "D")
        .filter_map(move |channel| {
            let (first, second) = channel.name.split_once("__")?;
            Some(if first == me { second } else { first }.to_owned())
        })
}

/// Fetches an opened channel's posts around the first unread.
pub(crate) async fn open_channel(
    client: Client,
    user_id: String,
    channel_id: String,
    collapsed_threads: bool,
    results: UnboundedSender<Loaded>,
) {
    let message = match client
        .posts_around_unread(
            &user_id,
            &channel_id,
            OPEN_LIMIT,
            OPEN_LIMIT,
            collapsed_threads,
        )
        .await
    {
        Ok(posts) => Loaded::Posts { channel_id, posts },
        Err(error) => Loaded::PostsFailed {
            channel_id,
            error: error.to_string(),
        },
    };
    let _ = results.send(message);
}

/// A `since` gap longer than this reloads the channel's latest page instead.
pub(crate) const MAX_GAP_MS: i64 = 6 * 60 * 60 * 1000;
/// Posts in the page that replaces a channel's cache after a long gap.
pub(crate) const RESET_PAGE: u32 = 60;

/// What a resync should catch up on.
#[derive(Debug)]
pub(crate) struct ResyncPlan {
    pub(crate) user_id: String,
    pub(crate) teams: Vec<String>,
    /// Channels with cached posts, most important first, with their `since`.
    pub(crate) channels: Vec<(String, Option<i64>)>,
    /// Under Collapsed Reply Threads, each team's thread-list `since`.
    pub(crate) threads: Vec<(String, Option<i64>)>,
    /// A `since` response bigger than this reloads the channel instead.
    pub(crate) max_response_bytes: usize,
}

/// Catches the cache up after missed events (#37): memberships and counts
/// per team, then `since` for each cached channel, then thread lists.
pub(crate) async fn resync(client: Client, plan: ResyncPlan, results: UnboundedSender<Loaded>) {
    let mut last_post_at = std::collections::HashMap::new();
    for team_id in &plan.teams {
        match team(&client, &plan.user_id, team_id).await {
            Ok(loaded) => {
                if let Loaded::Team { channels, .. } = &loaded {
                    for channel in channels {
                        last_post_at.insert(channel.id.clone(), channel.last_post_at);
                    }
                }
                let _ = results.send(loaded);
            }
            Err(error) => {
                let _ = results.send(Loaded::TeamFailed {
                    team_id: team_id.clone(),
                    error: error.to_string(),
                });
            }
        }
    }

    for (channel_id, since) in plan.channels {
        let newest = last_post_at.get(&channel_id).copied().unwrap_or(0);
        let message = match since {
            Some(since) if newest - since <= MAX_GAP_MS => {
                match since_page(&client, &channel_id, since, plan.max_response_bytes).await {
                    Ok(Some(posts)) => Ok(Loaded::Since {
                        channel_id: channel_id.clone(),
                        posts,
                    }),
                    Ok(None) => reset(&client, &channel_id).await,
                    Err(error) => Err(error),
                }
            }
            // No cursor, or too far behind: start the channel over.
            _ => reset(&client, &channel_id).await,
        };
        match message {
            Ok(message) => {
                let _ = results.send(message);
            }
            Err(error) => {
                let _ = results.send(Loaded::PostsFailed {
                    channel_id,
                    error: error.to_string(),
                });
            }
        }
    }

    for (team_id, since) in plan.threads {
        let query = ThreadsQuery {
            since,
            deleted: true,
            ..ThreadsQuery::default()
        };
        match client.user_threads(&plan.user_id, &team_id, &query).await {
            Ok(threads) => {
                let _ = results.send(Loaded::Threads {
                    team_id,
                    threads: threads.threads,
                });
            }
            Err(error) => tracing::warn!(%team_id, %error, "could not catch up threads"),
        }
    }
    let _ = results.send(Loaded::Resynced);
}

/// Every post changed in a channel since `since`, or `None` when the
/// response is over `max_bytes` (too much to merge; reload instead).
async fn since_page(
    client: &Client,
    channel_id: &str,
    since: i64,
    max_bytes: usize,
) -> Result<Option<PostList>, ApiError> {
    let path = format!("channels/{channel_id}/posts?since={since}");
    let bytes = match client.get_bytes(&path, None).await? {
        Fetched::Fresh { bytes, .. } => bytes,
        Fetched::NotModified => return Ok(Some(PostList::default())),
    };
    if bytes.len() > max_bytes {
        return Ok(None);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| ApiError::Decode(error.to_string()))
}

async fn reset(client: &Client, channel_id: &str) -> Result<Loaded, ApiError> {
    let posts = client
        .channel_posts(channel_id, &PostsQuery::latest(RESET_PAGE))
        .await?;
    Ok(Loaded::Reset {
        channel_id: channel_id.to_owned(),
        posts,
    })
}
