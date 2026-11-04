//! Posts, threads and Collapsed Reply Threads (PLAN §6.3 "Posts", "Writes").

use std::collections::HashMap;

use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    ApiError, Client,
    models::{
        ChannelUnreadAt, ChannelViewed, Draft, Post, PostActionResponse, PostList, Reaction,
        SubmitDialogRequest, SubmitDialogResponse, UserThread, UserThreads,
    },
};

/// Which page of `channels/{id}/posts` to fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PostsQuery {
    /// Posts older than this post id.
    pub before: Option<String>,
    /// Posts newer than this post id.
    pub after: Option<String>,
    /// Every post created, edited or deleted since this time (milliseconds
    /// since the epoch), including deleted ones; ignores paging.
    pub since: Option<i64>,
    /// Page number, from 0.
    pub page: u32,
    /// Posts per page; the server's maximum is 200.
    pub per_page: u32,
    /// Leave thread replies out (Collapsed Reply Threads).
    pub collapsed_threads: bool,
}

impl PostsQuery {
    /// The newest `per_page` posts.
    pub fn latest(per_page: u32) -> Self {
        Self {
            per_page,
            ..Self::default()
        }
    }

    /// `per_page` posts older than `post_id`.
    pub fn before(post_id: impl Into<String>, per_page: u32) -> Self {
        Self {
            before: Some(post_id.into()),
            per_page,
            ..Self::default()
        }
    }

    /// `per_page` posts newer than `post_id`.
    pub fn after(post_id: impl Into<String>, per_page: u32) -> Self {
        Self {
            after: Some(post_id.into()),
            per_page,
            ..Self::default()
        }
    }

    /// Everything changed since `millis`, for catching up after a reconnect.
    pub fn since(millis: i64) -> Self {
        Self {
            since: Some(millis),
            ..Self::default()
        }
    }

    /// Sets [`PostsQuery::collapsed_threads`].
    pub fn collapsed_threads(mut self, collapsed: bool) -> Self {
        self.collapsed_threads = collapsed;
        self
    }

    fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        if let Some(since) = self.since {
            params.push(("since", since.to_string()));
        } else {
            if let Some(before) = &self.before {
                params.push(("before", before.clone()));
            }
            if let Some(after) = &self.after {
                params.push(("after", after.clone()));
            }
            params.push(("page", self.page.to_string()));
            params.push(("per_page", self.per_page.max(1).to_string()));
        }
        if self.collapsed_threads {
            params.push(("collapsedThreads", "true".into()));
        }
        params
    }
}

/// Which way to page through a thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Older replies.
    Up,
    /// Newer replies.
    #[default]
    Down,
}

/// Which part of `posts/{id}/thread` to fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadQuery {
    /// Start after this reply's create time.
    pub from_create_at: Option<i64>,
    /// Start after this reply (with `from_create_at`, to break ties).
    pub from_post: Option<String>,
    /// Which way from the start point.
    pub direction: Direction,
    /// Replies per page; 0 fetches the whole thread.
    pub per_page: u32,
}

impl ThreadQuery {
    /// The whole thread.
    pub fn all() -> Self {
        Self::default()
    }

    /// The first `per_page` posts, oldest first.
    pub fn first(per_page: u32) -> Self {
        Self {
            per_page,
            ..Self::default()
        }
    }

    /// The newest `per_page` replies, with the root; page further back with
    /// [`ThreadQuery::from`] and [`Direction::Up`] while `has_next`.
    pub fn latest(per_page: u32) -> Self {
        Self {
            direction: Direction::Up,
            per_page,
            ..Self::default()
        }
    }

    /// `per_page` posts after `post` (created at `create_at`) in `direction`.
    pub fn from(
        post: impl Into<String>,
        create_at: i64,
        direction: Direction,
        per_page: u32,
    ) -> Self {
        Self {
            from_create_at: Some(create_at),
            from_post: Some(post.into()),
            direction,
            per_page,
        }
    }

    fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        if self.per_page > 0 {
            params.push(("perPage", self.per_page.to_string()));
            params.push((
                "direction",
                match self.direction {
                    Direction::Up => "up",
                    Direction::Down => "down",
                }
                .into(),
            ));
            params.push(("fromCreateAt", self.from_create_at.unwrap_or(0).to_string()));
            if let Some(post) = &self.from_post {
                params.push(("fromPost", post.clone()));
            }
        }
        params
    }
}

/// Which of the user's threads to fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadsQuery {
    /// Threads changed since this time (milliseconds since the epoch).
    pub since: Option<i64>,
    /// Only threads with unread replies.
    pub unread: bool,
    /// Threads older than this thread id.
    pub before: Option<String>,
    /// Threads newer than this thread id.
    pub after: Option<String>,
    /// Threads per page; 0 for the server's default (25).
    pub per_page: u32,
    /// Include threads whose root post was deleted.
    pub deleted: bool,
    /// Return participants as full users rather than ids.
    pub extended: bool,
}

impl ThreadsQuery {
    fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        if let Some(since) = self.since {
            params.push(("since", since.to_string()));
        }
        if self.unread {
            params.push(("unread", "true".into()));
        }
        if let Some(before) = &self.before {
            params.push(("before", before.clone()));
        }
        if let Some(after) = &self.after {
            params.push(("after", after.clone()));
        }
        if self.per_page > 0 {
            params.push(("pageSize", self.per_page.to_string()));
        }
        if self.deleted {
            params.push(("deleted", "true".into()));
        }
        if self.extended {
            params.push(("extended", "true".into()));
        }
        params
    }
}

/// A post to create.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NewPost {
    /// Channel.
    pub channel_id: String,
    /// Message text (Markdown).
    pub message: String,
    /// Thread root, for a reply.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub root_id: String,
    /// Uploaded file ids.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub file_ids: Vec<String>,
    /// Properties.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub props: HashMap<String, Value>,
    /// The outbox's id for this post (PLAN §6.5), echoed in the response
    /// and the `posted` event. Mattermost clients use `{user_id}:{millis}`.
    pub pending_post_id: String,
}

impl NewPost {
    /// A root post.
    pub fn new(
        channel_id: impl Into<String>,
        message: impl Into<String>,
        pending_post_id: impl Into<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            message: message.into(),
            pending_post_id: pending_post_id.into(),
            ..Self::default()
        }
    }

    /// Makes this a reply in `root_id`'s thread.
    pub fn reply_to(mut self, root_id: impl Into<String>) -> Self {
        self.root_id = root_id.into();
        self
    }
}

/// Changes to a post; fields left `None` are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PostPatch {
    /// New message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Pin or unpin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_pinned: Option<bool>,
    /// New attached files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_ids: Option<Vec<String>>,
    /// New properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub props: Option<HashMap<String, Value>>,
}

impl Client {
    /// `POST opengraph`: the server's Open Graph read of `url`, for a link
    /// preview while composing (#188). The server caches it.
    pub async fn opengraph(&self, url: &str) -> Result<crate::models::OpenGraph, ApiError> {
        self.post("opengraph", &serde_json::json!({ "url": url }))
            .await
    }

    /// `GET users/{user_id}/channels/{channel_id}/posts/unread`: the posts
    /// around the user's first unread one, for opening a channel there.
    pub async fn posts_around_unread(
        &self,
        user_id: &str,
        channel_id: &str,
        limit_before: u32,
        limit_after: u32,
        collapsed_threads: bool,
    ) -> Result<PostList, ApiError> {
        let mut query = vec![
            ("limit_before", limit_before.to_string()),
            ("limit_after", limit_after.to_string()),
        ];
        if collapsed_threads {
            query.push(("collapsedThreads", "true".into()));
        }
        self.get_query(
            &format!("users/{user_id}/channels/{channel_id}/posts/unread"),
            &query,
        )
        .await
    }

    /// `GET channels/{channel_id}/posts`.
    pub async fn channel_posts(
        &self,
        channel_id: &str,
        query: &PostsQuery,
    ) -> Result<PostList, ApiError> {
        self.get_query(&format!("channels/{channel_id}/posts"), &query.params())
            .await
    }

    /// `GET posts/{post_id}`.
    pub async fn get_post(&self, post_id: &str) -> Result<Post, ApiError> {
        self.get(&format!("posts/{post_id}")).await
    }

    /// `GET posts/{post_id}/thread`: the root and its replies, oldest first.
    pub async fn post_thread(
        &self,
        post_id: &str,
        query: &ThreadQuery,
    ) -> Result<PostList, ApiError> {
        self.get_query(&format!("posts/{post_id}/thread"), &query.params())
            .await
    }

    /// `GET users/{user_id}/teams/{team_id}/threads`.
    pub async fn user_threads(
        &self,
        user_id: &str,
        team_id: &str,
        query: &ThreadsQuery,
    ) -> Result<UserThreads, ApiError> {
        self.get_query(
            &format!("users/{user_id}/teams/{team_id}/threads"),
            &query.params(),
        )
        .await
    }

    /// `PUT users/{user_id}/teams/{team_id}/threads/{thread_id}/read/{millis}`:
    /// marks the thread read up to `millis`.
    pub async fn mark_thread_read(
        &self,
        user_id: &str,
        team_id: &str,
        thread_id: &str,
        millis: i64,
    ) -> Result<UserThread, ApiError> {
        self.put(
            &format!("users/{user_id}/teams/{team_id}/threads/{thread_id}/read/{millis}"),
            &json!({}),
        )
        .await
    }

    /// `PUT users/{user_id}/teams/{team_id}/threads/read`: marks every
    /// followed thread in the team read.
    pub async fn mark_all_threads_read(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<(), ApiError> {
        self.put::<_, Value>(
            &format!("users/{user_id}/teams/{team_id}/threads/read"),
            &json!({}),
        )
        .await
        .map(|_| ())
    }

    /// `POST users/{user_id}/posts/{post_id}/reminder`: the system bot
    /// reminds the user of the post at `target_time`, in Unix seconds.
    pub async fn set_post_reminder(
        &self,
        user_id: &str,
        post_id: &str,
        target_time: i64,
    ) -> Result<(), ApiError> {
        self.post::<_, Value>(
            &format!("users/{user_id}/posts/{post_id}/reminder"),
            &json!({ "target_time": target_time }),
        )
        .await
        .map(|_| ())
    }

    /// `POST posts/{post_id}/actions/{action_id}`: presses an interactive
    /// message's button, or picks `selected_option` from its menu. The
    /// server passes the choice on to the integration. `cookie` is the
    /// action's [`cookie`](crate::models::PostAction::cookie), which an
    /// ephemeral post needs; empty leaves it out.
    pub async fn do_post_action(
        &self,
        post_id: &str,
        action_id: &str,
        selected_option: Option<&str>,
        cookie: &str,
    ) -> Result<PostActionResponse, ApiError> {
        let mut body = serde_json::Map::new();
        if let Some(option) = selected_option {
            body.insert("selected_option".into(), json!(option));
        }
        if !cookie.is_empty() {
            body.insert("cookie".into(), json!(cookie));
        }
        self.post(
            &format!("posts/{post_id}/actions/{action_id}"),
            &Value::Object(body),
        )
        .await
    }

    /// `POST actions/dialogs/submit`: sends an interactive dialog's values,
    /// or with `cancelled` set, that the user closed it. The integration's
    /// complaints come back in the answer; a server that answers with no
    /// body took it.
    pub async fn submit_dialog(
        &self,
        submission: &SubmitDialogRequest,
    ) -> Result<SubmitDialogResponse, ApiError> {
        let body = serde_json::to_vec(submission).map_err(|e| ApiError::Decode(e.to_string()))?;
        let response = self
            .send(Method::POST, "actions/dialogs/submit", &[], Some(body))
            .await?;
        let bytes = response.bytes().await.map_err(ApiError::from_transport)?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(SubmitDialogResponse::default());
        }
        serde_json::from_slice::<Option<SubmitDialogResponse>>(&bytes)
            .map(Option::unwrap_or_default)
            .map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// `PUT users/{user_id}/teams/{team_id}/threads/{thread_id}/following`.
    pub async fn follow_thread(
        &self,
        user_id: &str,
        team_id: &str,
        thread_id: &str,
    ) -> Result<(), ApiError> {
        self.put::<_, Value>(
            &format!("users/{user_id}/teams/{team_id}/threads/{thread_id}/following"),
            &json!({}),
        )
        .await
        .map(|_| ())
    }

    /// `DELETE users/{user_id}/teams/{team_id}/threads/{thread_id}/following`.
    pub async fn unfollow_thread(
        &self,
        user_id: &str,
        team_id: &str,
        thread_id: &str,
    ) -> Result<(), ApiError> {
        self.delete::<Value>(&format!(
            "users/{user_id}/teams/{team_id}/threads/{thread_id}/following"
        ))
        .await
        .map(|_| ())
    }

    /// `POST posts`.
    pub async fn create_post(&self, post: &NewPost) -> Result<Post, ApiError> {
        self.post("posts", post).await
    }

    /// `PUT posts/{post_id}/patch`.
    pub async fn patch_post(&self, post_id: &str, patch: &PostPatch) -> Result<Post, ApiError> {
        self.put(&format!("posts/{post_id}/patch"), patch).await
    }

    /// `DELETE posts/{post_id}`.
    pub async fn delete_post(&self, post_id: &str) -> Result<(), ApiError> {
        self.delete::<Value>(&format!("posts/{post_id}"))
            .await
            .map(|_| ())
    }

    /// `POST channels/members/me/view`: marks `channel_id` read, and
    /// `prev_channel_id` (the channel the user left, may be empty) too.
    pub async fn view_channel(
        &self,
        channel_id: &str,
        prev_channel_id: &str,
    ) -> Result<ChannelViewed, ApiError> {
        self.post(
            "channels/members/me/view",
            &json!({
                "channel_id": channel_id,
                "prev_channel_id": prev_channel_id,
                "collapsed_threads_supported": true,
            }),
        )
        .await
    }

    /// `POST users/{user_id}/posts/{post_id}/set_unread`: marks the channel
    /// (or, with Collapsed Reply Threads, the thread) unread from `post_id`.
    pub async fn set_post_unread(
        &self,
        user_id: &str,
        post_id: &str,
        collapsed_threads_supported: bool,
    ) -> Result<ChannelUnreadAt, ApiError> {
        self.post(
            &format!("users/{user_id}/posts/{post_id}/set_unread"),
            &json!({ "collapsed_threads_supported": collapsed_threads_supported }),
        )
        .await
    }

    /// `POST reactions`: adds `emoji_name` from `user_id` to `post_id`.
    pub async fn add_reaction(
        &self,
        user_id: &str,
        post_id: &str,
        emoji_name: &str,
    ) -> Result<Reaction, ApiError> {
        self.post(
            "reactions",
            &json!({ "user_id": user_id, "post_id": post_id, "emoji_name": emoji_name }),
        )
        .await
    }

    /// `DELETE users/{user_id}/posts/{post_id}/reactions/{emoji_name}`.
    pub async fn remove_reaction(
        &self,
        user_id: &str,
        post_id: &str,
        emoji_name: &str,
    ) -> Result<(), ApiError> {
        self.delete::<Value>(&format!(
            "users/{user_id}/posts/{post_id}/reactions/{emoji_name}"
        ))
        .await
        .map(|_| ())
    }

    /// `POST drafts`: creates or replaces the draft for its channel and root.
    /// Only when the server allows synced drafts.
    pub async fn save_draft(&self, draft: &Draft) -> Result<Draft, ApiError> {
        self.post("drafts", draft).await
    }

    /// `GET users/{user_id}/teams/{team_id}/drafts`. The server answers
    /// `null` for none.
    pub async fn drafts(&self, user_id: &str, team_id: &str) -> Result<Vec<Draft>, ApiError> {
        self.get::<Option<Vec<Draft>>>(&format!("users/{user_id}/teams/{team_id}/drafts"))
            .await
            .map(Option::unwrap_or_default)
    }

    /// `DELETE users/{user_id}/channels/{channel_id}/drafts[/{root_id}]`.
    pub async fn delete_draft(
        &self,
        user_id: &str,
        channel_id: &str,
        root_id: &str,
    ) -> Result<(), ApiError> {
        let mut path = format!("users/{user_id}/channels/{channel_id}/drafts");
        if !root_id.is_empty() {
            path.push('/');
            path.push_str(root_id);
        }
        self.delete::<Value>(&path).await.map(|_| ())
    }
}
