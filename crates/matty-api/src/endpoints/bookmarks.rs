//! Channel bookmarks (#226): `channels/{channel_id}/bookmarks`.

use reqwest::Method;

use crate::{
    ApiError, Client,
    models::{BookmarkPatch, BookmarkUpdate, ChannelBookmark, NewBookmark},
};

impl Client {
    /// `GET channels/{channel_id}/bookmarks`: the channel's bookmarks, in
    /// no particular order.
    pub async fn channel_bookmarks(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelBookmark>, ApiError> {
        let list: Option<Vec<ChannelBookmark>> = self
            .get(&format!("channels/{channel_id}/bookmarks"))
            .await?;
        Ok(list.unwrap_or_default())
    }

    /// `POST channels/{channel_id}/bookmarks`.
    pub async fn create_bookmark(
        &self,
        bookmark: &NewBookmark,
    ) -> Result<ChannelBookmark, ApiError> {
        self.post(
            &format!("channels/{}/bookmarks", bookmark.channel_id),
            bookmark,
        )
        .await
    }

    /// `PATCH channels/{channel_id}/bookmarks/{bookmark_id}`.
    pub async fn patch_bookmark(
        &self,
        channel_id: &str,
        bookmark_id: &str,
        patch: &BookmarkPatch,
    ) -> Result<BookmarkUpdate, ApiError> {
        let body = serde_json::to_vec(patch).map_err(|e| ApiError::Decode(e.to_string()))?;
        let response = self
            .send(
                Method::PATCH,
                &format!("channels/{channel_id}/bookmarks/{bookmark_id}"),
                &[],
                Some(body),
            )
            .await?;
        let bytes = response.bytes().await.map_err(ApiError::from_transport)?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// `DELETE channels/{channel_id}/bookmarks/{bookmark_id}`: answers
    /// with the bookmark as deleted.
    pub async fn delete_bookmark(
        &self,
        channel_id: &str,
        bookmark_id: &str,
    ) -> Result<ChannelBookmark, ApiError> {
        self.delete(&format!("channels/{channel_id}/bookmarks/{bookmark_id}"))
            .await
    }

    /// `POST channels/{channel_id}/bookmarks/{bookmark_id}/sort_order`:
    /// moves a bookmark to `index` in the bar; answers with every bookmark
    /// whose place changed.
    pub async fn sort_bookmark(
        &self,
        channel_id: &str,
        bookmark_id: &str,
        index: i64,
    ) -> Result<Vec<ChannelBookmark>, ApiError> {
        self.post(
            &format!("channels/{channel_id}/bookmarks/{bookmark_id}/sort_order"),
            &index,
        )
        .await
    }
}
