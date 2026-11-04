//! M1 features: search, saved and pinned posts, browsing and joining
//! channels, direct and group messages, reactions and autocomplete
//! (PLAN §6.3 "M1 feature endpoints").

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    ApiError, Client,
    models::{Channel, ChannelMember, PostList, Reaction, SearchResults, User, UserAutocomplete},
};

/// A channel to create, as `POST channels` takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NewChannel {
    /// The team it belongs to.
    pub team_id: String,
    /// The URL handle: lowercase letters, digits, `-` and `_`.
    pub name: String,
    /// The name people see.
    pub display_name: String,
    /// What the channel is for; may be empty.
    pub purpose: String,
    /// `O` public or `P` private.
    #[serde(rename = "type")]
    pub kind: String,
}

/// The fields of a channel to change, as `PUT channels/{id}/patch` takes
/// them; `None` leaves a field alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ChannelPatch {
    /// The name people see.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The header text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// What the channel is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// The fields of the user's own profile to change, as `PUT users/{id}/patch`
/// takes them; `None` leaves a field alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ProfilePatch {
    /// Given name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    /// Family name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    /// Nickname.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    /// Job title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// Without `@`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl ProfilePatch {
    /// Whether the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A post search, as `POST teams/{id}/posts/search` takes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchQuery {
    /// The search text, with Mattermost's modifiers (`from:`, `in:`,
    /// `before:`, quotes, `-exclude`, `*` suffixes).
    pub terms: String,
    /// Match any word instead of all of them.
    pub is_or_search: bool,
    /// The user's offset from UTC in seconds, for `on:`/`before:`/`after:`.
    pub time_zone_offset: i32,
    /// Page number, from 0.
    pub page: u32,
    /// Results per page.
    pub per_page: u32,
    /// Also search archived channels.
    pub include_deleted_channels: bool,
}

impl SearchQuery {
    /// The first page of 20 results for `terms`, matching all words.
    pub fn new(terms: impl Into<String>) -> Self {
        Self {
            terms: terms.into(),
            is_or_search: false,
            time_zone_offset: 0,
            page: 0,
            per_page: 20,
            include_deleted_channels: false,
        }
    }
}

impl Client {
    /// `POST teams/{team_id}/posts/search`.
    pub async fn search_posts(
        &self,
        team_id: &str,
        query: &SearchQuery,
    ) -> Result<SearchResults, ApiError> {
        self.post(&format!("teams/{team_id}/posts/search"), query)
            .await
    }

    /// `GET users/{user_id}/posts/flagged`: saved posts, newest first,
    /// across all teams.
    pub async fn saved_posts(
        &self,
        user_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<PostList, ApiError> {
        self.get_query(
            &format!("users/{user_id}/posts/flagged"),
            &[
                ("page", page.to_string()),
                ("per_page", per_page.to_string()),
            ],
        )
        .await
    }

    /// `GET channels/{channel_id}/pinned`.
    pub async fn pinned_posts(&self, channel_id: &str) -> Result<PostList, ApiError> {
        self.get(&format!("channels/{channel_id}/pinned")).await
    }

    /// `POST posts/{post_id}/pin`.
    pub async fn pin_post(&self, post_id: &str) -> Result<(), ApiError> {
        self.post::<_, Value>(&format!("posts/{post_id}/pin"), &json!({}))
            .await
            .map(|_| ())
    }

    /// `POST posts/{post_id}/unpin`.
    pub async fn unpin_post(&self, post_id: &str) -> Result<(), ApiError> {
        self.post::<_, Value>(&format!("posts/{post_id}/unpin"), &json!({}))
            .await
            .map(|_| ())
    }

    /// `GET posts/{post_id}/reactions`. The server answers `null` for none.
    pub async fn post_reactions(&self, post_id: &str) -> Result<Vec<Reaction>, ApiError> {
        self.get::<Option<Vec<Reaction>>>(&format!("posts/{post_id}/reactions"))
            .await
            .map(Option::unwrap_or_default)
    }

    /// `GET teams/{team_id}/channels`: one page of the team's public
    /// channels, for browsing.
    pub async fn public_channels(
        &self,
        team_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<Channel>, ApiError> {
        self.get_query(
            &format!("teams/{team_id}/channels"),
            &[
                ("page", page.to_string()),
                ("per_page", per_page.to_string()),
            ],
        )
        .await
    }

    /// `POST channels/stats/member_count`: member counts by channel id.
    pub async fn channel_member_counts(
        &self,
        channel_ids: &[String],
    ) -> Result<std::collections::HashMap<String, i64>, ApiError> {
        self.post("channels/stats/member_count", channel_ids).await
    }

    /// `PUT users/{user_id}/patch`: changes the user's names, nickname,
    /// position or username, and returns the updated user.
    pub async fn patch_profile(
        &self,
        user_id: &str,
        patch: &ProfilePatch,
    ) -> Result<crate::models::User, ApiError> {
        self.put(&format!("users/{user_id}/patch"), patch).await
    }

    /// `PUT channels/{channel_id}/patch`: renames a channel or changes its
    /// header or purpose.
    pub async fn patch_channel(
        &self,
        channel_id: &str,
        patch: &ChannelPatch,
    ) -> Result<Channel, ApiError> {
        self.put(&format!("channels/{channel_id}/patch"), patch)
            .await
    }

    /// `DELETE channels/{channel_id}`: archives the channel. Its posts stay
    /// on the server.
    pub async fn archive_channel(&self, channel_id: &str) -> Result<(), ApiError> {
        self.delete::<Value>(&format!("channels/{channel_id}"))
            .await
            .map(|_| ())
    }

    /// `PUT channels/{channel_id}/privacy`: makes a public channel private
    /// (`private`) or a private one public.
    pub async fn set_channel_privacy(
        &self,
        channel_id: &str,
        private: bool,
    ) -> Result<Channel, ApiError> {
        let privacy = if private { "P" } else { "O" };
        self.put(
            &format!("channels/{channel_id}/privacy"),
            &json!({ "privacy": privacy }),
        )
        .await
    }

    /// `PUT channels/{channel_id}/members/{user_id}/notify_props`: changes
    /// these of the user's notification properties for the channel.
    pub async fn update_channel_notify_props(
        &self,
        channel_id: &str,
        user_id: &str,
        props: &std::collections::HashMap<String, String>,
    ) -> Result<(), ApiError> {
        self.put::<_, Value>(
            &format!("channels/{channel_id}/members/{user_id}/notify_props"),
            props,
        )
        .await
        .map(|_| ())
    }

    /// `POST channels`: creates a public (`O`) or private (`P`) channel the
    /// user is then a member of.
    pub async fn create_channel(&self, channel: &NewChannel) -> Result<Channel, ApiError> {
        self.post("channels", channel).await
    }

    /// `GET users?in_team=&active=true`: one page of a team's people, for
    /// starting a direct message.
    pub async fn team_users(
        &self,
        team_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<User>, ApiError> {
        self.get_query(
            "users",
            &[
                ("in_team", team_id.to_owned()),
                ("active", "true".to_owned()),
                ("page", page.to_string()),
                ("per_page", per_page.to_string()),
            ],
        )
        .await
    }

    /// `POST teams/{team_id}/channels/search`: public channels whose name
    /// matches `term`.
    pub async fn search_channels(
        &self,
        team_id: &str,
        term: &str,
    ) -> Result<Vec<Channel>, ApiError> {
        self.post(
            &format!("teams/{team_id}/channels/search"),
            &json!({ "term": term }),
        )
        .await
    }

    /// `GET teams/{team_id}/channels/autocomplete`: channels for `~name`.
    pub async fn autocomplete_channels(
        &self,
        team_id: &str,
        name: &str,
    ) -> Result<Vec<Channel>, ApiError> {
        self.get_query(
            &format!("teams/{team_id}/channels/autocomplete"),
            &[("name", name.to_owned())],
        )
        .await
    }

    /// `POST channels/{channel_id}/members`: joins a public channel, or adds
    /// someone to a channel.
    pub async fn add_channel_member(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> Result<ChannelMember, ApiError> {
        self.post(
            &format!("channels/{channel_id}/members"),
            &json!({ "user_id": user_id }),
        )
        .await
    }

    /// `DELETE channels/{channel_id}/members/{user_id}`: leaves a channel,
    /// or removes someone from it.
    pub async fn remove_channel_member(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> Result<(), ApiError> {
        self.delete::<Value>(&format!("channels/{channel_id}/members/{user_id}"))
            .await
            .map(|_| ())
    }

    /// `POST channels/direct`: the direct message channel between two users,
    /// created if needed. Pass the same id twice for a self-DM.
    pub async fn direct_channel(&self, user_id: &str, other_id: &str) -> Result<Channel, ApiError> {
        self.post("channels/direct", &[user_id, other_id]).await
    }

    /// `POST channels/group`: the group message channel of 3 to 8 users,
    /// including the caller, created if needed.
    pub async fn group_channel(&self, user_ids: &[String]) -> Result<Channel, ApiError> {
        self.post("channels/group", user_ids).await
    }

    /// `GET users/autocomplete`: users for `@name`, those in `channel_id`
    /// first when one is given.
    pub async fn autocomplete_users(
        &self,
        name: &str,
        team_id: Option<&str>,
        channel_id: Option<&str>,
    ) -> Result<UserAutocomplete, ApiError> {
        let mut query = vec![("name", name.to_owned())];
        if let Some(team) = team_id {
            query.push(("in_team", team.to_owned()));
        }
        if let Some(channel) = channel_id {
            query.push(("in_channel", channel.to_owned()));
        }
        self.get_query("users/autocomplete", &query).await
    }
}
