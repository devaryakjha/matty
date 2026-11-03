//! Initial load (PLAN §6.3): what the engine fetches on connect. Fetching
//! runs in tasks on the sync runtime; every result comes back to the engine
//! loop as a [`Loaded`] message, which writes it in one transaction.

use std::collections::BTreeSet;

use matty_api::models::{
    Channel, ChannelMember, Emoji, Preference, SidebarCategories, Status, Team, User,
};
use matty_api::{ApiError, Client, ServerCapabilities};
use tokio::sync::mpsc::UnboundedSender;

/// Posts fetched on each side of the first unread when a channel opens.
pub(crate) const OPEN_LIMIT: u32 = 60;

/// A result of the load, for the engine to write.
#[derive(Debug)]
pub(crate) enum Loaded {
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
    /// The account itself could not load.
    Failed(String),
    /// Custom emoji.
    Emoji(Vec<Emoji>),
    /// Statuses for the people in direct messages.
    Statuses(Vec<Status>),
    /// Every team has been tried.
    Done,
    /// An opened channel's posts around the first unread.
    Posts {
        channel_id: String,
        posts: matty_api::models::PostList,
    },
    /// An opened channel's posts could not load.
    PostsFailed { channel_id: String, error: String },
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
