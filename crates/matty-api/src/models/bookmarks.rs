//! Channel bookmarks (#226): links and files pinned to a channel's header,
//! from `channels/{channel_id}/bookmarks` on Mattermost 9.5 and later.

use serde::{Deserialize, Serialize};

use super::FileInfo;

/// What a bookmark points at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BookmarkKind {
    /// A web address in [`ChannelBookmark::link_url`].
    #[default]
    Link,
    /// An uploaded file, [`ChannelBookmark::file_id`].
    File,
}

/// One bookmark, with its file when it is a file bookmark
/// (`ChannelBookmarkWithFileInfo`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ChannelBookmark {
    /// Bookmark id. Editing someone else's bookmark gives it a new id.
    pub id: String,
    /// Milliseconds since the epoch.
    pub create_at: i64,
    /// Milliseconds since the epoch.
    pub update_at: i64,
    /// Milliseconds since the epoch; nonzero once deleted.
    pub delete_at: i64,
    /// The channel it belongs to.
    pub channel_id: String,
    /// Who added it, or last edited it.
    pub owner_id: String,
    /// The file, for a file bookmark; empty for a link.
    pub file_id: String,
    /// The name shown in the bar.
    pub display_name: String,
    /// Its place in the bar, from 0.
    pub sort_order: i64,
    /// The address, for a link bookmark.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub link_url: String,
    /// The link's icon (its favicon or Open Graph image), if any.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image_url: String,
    /// An emoji shown instead of the icon, by name without colons.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub emoji: String,
    /// Link or file.
    #[serde(rename = "type")]
    pub kind: BookmarkKind,
    /// The bookmark this one replaced when someone edited another
    /// person's bookmark.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub original_id: String,
    /// Unused by the server today.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent_id: String,
    /// The file, for a file bookmark.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileInfo>,
}

/// A bookmark to create, as `POST channels/{channel_id}/bookmarks` takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NewBookmark {
    /// The channel.
    pub channel_id: String,
    /// The name shown in the bar.
    pub display_name: String,
    /// Link or file.
    #[serde(rename = "type")]
    pub kind: BookmarkKind,
    /// The address, for a link.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub link_url: String,
    /// An uploaded file's id, for a file.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file_id: String,
    /// An emoji name without colons, shown instead of the icon.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub emoji: String,
}

/// The fields of a bookmark to change, as
/// `PATCH channels/{channel_id}/bookmarks/{bookmark_id}` takes them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BookmarkPatch {
    /// The name shown in the bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The address, for a link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
    /// An emoji name without colons; empty removes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
}

/// What a bookmark edit answers, and what `channel_bookmark_updated`
/// carries: the bookmark as it now stands and, when editing someone else's
/// bookmark replaced it, the one it replaced.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BookmarkUpdate {
    /// The bookmark now.
    pub updated: Option<ChannelBookmark>,
    /// The bookmark it replaced, now deleted.
    pub deleted: Option<ChannelBookmark>,
}

/// Sorts bookmarks into bar order: by `sort_order`, then age.
pub fn sort_bookmarks(bookmarks: &mut [ChannelBookmark]) {
    bookmarks.sort_by(|a, b| {
        (a.sort_order, a.create_at, &a.id).cmp(&(b.sort_order, b.create_at, &b.id))
    });
}
