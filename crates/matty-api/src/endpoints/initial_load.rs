//! What Matty loads at connect time (PLAN §6.3 "Initial load", "Users").

use serde_json::{Value, json};

use crate::{
    ApiError, Client, Fetched,
    models::{
        Channel, ChannelMember, ChannelStats, Emoji, Preference, SidebarCategories,
        SidebarCategory, Status, Team, TeamMember, TeamUnread, User,
    },
};

/// Page size for the paged helpers; the server's maximum is 200.
pub const PAGE_SIZE: usize = 200;

impl Client {
    /// `GET users/me/teams`.
    pub async fn my_teams(&self) -> Result<Vec<Team>, ApiError> {
        self.get("users/me/teams").await
    }

    /// `GET users/me/teams/members`.
    pub async fn my_team_members(&self) -> Result<Vec<TeamMember>, ApiError> {
        self.get("users/me/teams/members").await
    }

    /// `GET users/{user_id}/teams/unread`: badge counts for every team.
    pub async fn team_unreads(&self, user_id: &str) -> Result<Vec<TeamUnread>, ApiError> {
        self.get(&format!("users/{user_id}/teams/unread")).await
    }

    /// `GET users/me/teams/{team_id}/channels`, including DMs and group messages.
    pub async fn my_channels(&self, team_id: &str) -> Result<Vec<Channel>, ApiError> {
        self.get(&format!("users/me/teams/{team_id}/channels"))
            .await
    }

    /// `GET users/{user_id}/teams/{team_id}/channels/members`: unread state
    /// for every channel the user is in.
    pub async fn my_channel_members(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Vec<ChannelMember>, ApiError> {
        self.get(&format!("users/{user_id}/teams/{team_id}/channels/members"))
            .await
    }

    /// `GET users/me/teams/{team_id}/channels/categories`.
    pub async fn sidebar_categories(&self, team_id: &str) -> Result<SidebarCategories, ApiError> {
        self.get(&format!("users/me/teams/{team_id}/channels/categories"))
            .await
    }

    /// `POST users/me/teams/{team_id}/channels/categories`: a new custom
    /// category, with any `channel_ids` moved into it.
    pub async fn create_category(
        &self,
        team_id: &str,
        category: &SidebarCategory,
    ) -> Result<SidebarCategory, ApiError> {
        self.post(
            &format!("users/me/teams/{team_id}/channels/categories"),
            category,
        )
        .await
    }

    /// `PUT users/me/teams/{team_id}/channels/categories/order`: the team's
    /// categories in this order; returns the order the server kept.
    pub async fn set_category_order(
        &self,
        team_id: &str,
        order: &[String],
    ) -> Result<Vec<String>, ApiError> {
        self.put(
            &format!("users/me/teams/{team_id}/channels/categories/order"),
            order,
        )
        .await
    }

    /// `PUT users/me/teams/{team_id}/channels/categories`: saves
    /// `categories` whole. A channel listed in one leaves the category it
    /// was in.
    pub async fn update_categories(
        &self,
        team_id: &str,
        categories: &[SidebarCategory],
    ) -> Result<Vec<SidebarCategory>, ApiError> {
        self.put(
            &format!("users/me/teams/{team_id}/channels/categories"),
            categories,
        )
        .await
    }

    /// `DELETE users/me/teams/{team_id}/channels/categories/{category_id}`;
    /// its channels go back to their default categories.
    pub async fn delete_category(&self, team_id: &str, category_id: &str) -> Result<(), ApiError> {
        self.delete::<Value>(&format!(
            "users/me/teams/{team_id}/channels/categories/{category_id}"
        ))
        .await
        .map(|_| ())
    }

    /// `GET users/me/preferences`.
    pub async fn preferences(&self) -> Result<Vec<Preference>, ApiError> {
        self.get("users/me/preferences").await
    }

    /// `PUT users/{user_id}/preferences`.
    pub async fn save_preferences(
        &self,
        user_id: &str,
        preferences: &[Preference],
    ) -> Result<(), ApiError> {
        self.put::<_, Value>(&format!("users/{user_id}/preferences"), preferences)
            .await
            .map(|_| ())
    }

    /// `POST users/{user_id}/preferences/delete`.
    pub async fn delete_preferences(
        &self,
        user_id: &str,
        preferences: &[Preference],
    ) -> Result<(), ApiError> {
        self.post::<_, Value>(&format!("users/{user_id}/preferences/delete"), preferences)
            .await
            .map(|_| ())
    }

    /// `GET emoji?page=&per_page=`: one page of custom emoji.
    pub async fn emoji_page(&self, page: usize) -> Result<Vec<Emoji>, ApiError> {
        self.get_query(
            "emoji",
            &[
                ("page", page.to_string()),
                ("per_page", PAGE_SIZE.to_string()),
            ],
        )
        .await
    }

    /// Every custom emoji, paging until the server returns a short page.
    pub async fn all_emoji(&self) -> Result<Vec<Emoji>, ApiError> {
        collect_pages(|page| self.emoji_page(page)).await
    }

    /// `GET users?page=&per_page=`: one page of users.
    pub async fn users_page(&self, page: usize) -> Result<Vec<User>, ApiError> {
        self.get_query(
            "users",
            &[
                ("page", page.to_string()),
                ("per_page", PAGE_SIZE.to_string()),
            ],
        )
        .await
    }

    /// Every user the session can see, paging until a short page.
    pub async fn all_users(&self) -> Result<Vec<User>, ApiError> {
        collect_pages(|page| self.users_page(page)).await
    }

    /// `POST users/ids`.
    pub async fn users_by_ids(&self, ids: &[String]) -> Result<Vec<User>, ApiError> {
        self.post("users/ids", ids).await
    }

    /// `POST users/usernames`.
    pub async fn users_by_usernames(&self, usernames: &[String]) -> Result<Vec<User>, ApiError> {
        self.post("users/usernames", usernames).await
    }

    /// `GET users/{user_id}`.
    pub async fn user(&self, user_id: &str) -> Result<User, ApiError> {
        self.get(&format!("users/{user_id}")).await
    }

    /// `POST users/status/ids`.
    pub async fn statuses(&self, user_ids: &[String]) -> Result<Vec<Status>, ApiError> {
        self.post("users/status/ids", user_ids).await
    }

    /// `PUT users/{user_id}/status`. `dnd_end_time` is seconds since the
    /// epoch for Do Not Disturb with a duration.
    pub async fn set_status(
        &self,
        user_id: &str,
        status: &str,
        dnd_end_time: Option<i64>,
    ) -> Result<Status, ApiError> {
        let mut body = json!({ "user_id": user_id, "status": status });
        if let Some(end) = dnd_end_time {
            body["dnd_end_time"] = end.into();
        }
        self.put(&format!("users/{user_id}/status"), &body).await
    }

    /// `PUT users/{user_id}/patch` with the whole `notify_props` map: the
    /// server replaces the map, so it carries every key, changed or not.
    pub async fn patch_notify_props(
        &self,
        user_id: &str,
        notify_props: &std::collections::HashMap<String, String>,
    ) -> Result<User, ApiError> {
        self.put(
            &format!("users/{user_id}/patch"),
            &json!({ "notify_props": notify_props }),
        )
        .await
    }

    /// `GET channels/{channel_id}`.
    pub async fn channel(&self, channel_id: &str) -> Result<Channel, ApiError> {
        self.get(&format!("channels/{channel_id}")).await
    }

    /// `GET teams/{team_id}/channels/name/{name}`: a channel by its URL
    /// name, as channel links carry it. Archived channels count.
    pub async fn channel_by_name(&self, team_id: &str, name: &str) -> Result<Channel, ApiError> {
        let name: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        self.get_query(
            &format!("teams/{team_id}/channels/name/{name}"),
            &[("include_deleted", "true".to_owned())],
        )
        .await
    }

    /// `GET channels/{channel_id}/members/me`: the user's own membership.
    pub async fn my_channel_member(&self, channel_id: &str) -> Result<ChannelMember, ApiError> {
        self.get(&format!("channels/{channel_id}/members/me")).await
    }

    /// `GET channels/{channel_id}/stats`.
    pub async fn channel_stats(&self, channel_id: &str) -> Result<ChannelStats, ApiError> {
        self.get(&format!("channels/{channel_id}/stats")).await
    }

    /// `GET users/{user_id}/image`, conditional on the cached `etag`. Cache
    /// avatars by user id and `last_picture_update`.
    pub async fn avatar(&self, user_id: &str, etag: Option<&str>) -> Result<Fetched, ApiError> {
        self.get_bytes(&format!("users/{user_id}/image"), etag)
            .await
    }
}

/// Calls `page(0)`, `page(1)`, ... until a page comes back shorter than
/// [`PAGE_SIZE`], and returns everything.
async fn collect_pages<T, F, Fut>(mut page: F) -> Result<Vec<T>, ApiError>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>, ApiError>>,
{
    let mut all = Vec::new();
    for index in 0.. {
        let items = page(index).await?;
        let last = items.len() < PAGE_SIZE;
        all.extend(items);
        if last {
            break;
        }
    }
    Ok(all)
}
