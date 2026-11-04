//! Files: upload with progress, and the bytes of files, thumbnails,
//! previews, emoji and proxied images (PLAN §2, §6.3, §6.6).
//!
//! Every download returns the bytes with their content type, and, like every
//! request, never follows a redirect to another host.

use std::sync::Arc;

use futures_util::stream;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};

use crate::{ApiError, Client, Fetched, models::FileInfo};

/// Bytes handed to the connection between progress reports.
const CHUNK: usize = 64 * 1024;

/// A file to upload into a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    /// The channel the file will be posted in.
    pub channel_id: String,
    /// Matty's id for this upload, echoed back in [`Uploaded::client_ids`].
    pub client_id: String,
    /// The file name the server stores and shows.
    pub file_name: String,
    /// The file's contents.
    pub bytes: Vec<u8>,
}

/// The answer to `POST files`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Uploaded {
    /// The stored files; their ids go in the post's `file_ids`.
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub file_infos: Vec<FileInfo>,
    /// The client ids sent, in the same order.
    #[serde(deserialize_with = "crate::models::null_as_default")]
    pub client_ids: Vec<String>,
}

/// Reports upload progress: bytes of the file sent so far, and the total.
/// Called from the connection's task, so keep it quick.
pub type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Deserialize)]
struct PublicLink {
    link: String,
}

impl Client {
    /// `POST files`: uploads one file, streaming it and calling `progress` as
    /// it goes, with sent bytes rising to the total. Dropping the future
    /// cancels the upload and closes its connection.
    pub async fn upload_file(
        &self,
        upload: Upload,
        progress: Progress,
    ) -> Result<Uploaded, ApiError> {
        let total = upload.bytes.len() as u64;
        let bytes = bytes::Bytes::from(upload.bytes);
        let mut sent = 0u64;
        progress(0, total);
        let chunks = stream::iter((0..bytes.len()).step_by(CHUNK).map(move |start| {
            let chunk = bytes.slice(start..(start + CHUNK).min(bytes.len()));
            sent += chunk.len() as u64;
            progress(sent, total);
            Ok::<_, std::io::Error>(chunk)
        }));
        #[cfg(not(target_family = "wasm"))]
        let file = Part::stream_with_length(reqwest::Body::wrap_stream(chunks), total)
            .file_name(upload.file_name);
        // fetch() takes the whole body at once; progress jumps to done.
        #[cfg(target_family = "wasm")]
        let file = {
            let mut chunks = chunks;
            let mut all = Vec::with_capacity(total as usize);
            while let Some(Ok(chunk)) = futures_util::StreamExt::next(&mut chunks).await {
                all.extend_from_slice(&chunk);
            }
            Part::bytes(all).file_name(upload.file_name)
        };
        // The server reads fields in order: the channel must come first.
        let form = Form::new()
            .text("channel_id", upload.channel_id)
            .text("client_ids", upload.client_id)
            .part("files", file);
        let response = self.send_multipart("files", form).await?;
        let body = response.bytes().await.map_err(ApiError::from_transport)?;
        serde_json::from_slice(&body).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// `GET files/{file_id}`: the file itself.
    pub async fn file(&self, file_id: &str, etag: Option<&str>) -> Result<Fetched, ApiError> {
        self.get_bytes(&format!("files/{file_id}"), etag).await
    }

    /// `GET files/{file_id}` as it arrives, for saving a file (#151): hands
    /// each piece of the body to `chunk` with the bytes received so far and
    /// the length the server gave, if any. An error from `chunk` stops the
    /// download. Allows a long transfer rather than the usual timeout, and
    /// returns the bytes received. Not in the browser, which saves files
    /// itself.
    #[cfg(not(target_family = "wasm"))]
    pub async fn download_file(
        &self,
        file_id: &str,
        mut chunk: impl FnMut(&[u8], u64, Option<u64>) -> std::io::Result<()>,
    ) -> Result<u64, ApiError> {
        let mut response = self.send_long_get(&format!("files/{file_id}")).await?;
        let total = response.content_length();
        let mut received = 0u64;
        while let Some(piece) = response.chunk().await.map_err(ApiError::from_transport)? {
            received += piece.len() as u64;
            chunk(&piece, received, total).map_err(|error| ApiError::Io(error.to_string()))?;
        }
        Ok(received)
    }

    /// `GET files/{file_id}/thumbnail`: a small JPEG of an image.
    pub async fn file_thumbnail(
        &self,
        file_id: &str,
        etag: Option<&str>,
    ) -> Result<Fetched, ApiError> {
        self.get_bytes(&format!("files/{file_id}/thumbnail"), etag)
            .await
    }

    /// `GET files/{file_id}/preview`: an image scaled for the timeline.
    pub async fn file_preview(
        &self,
        file_id: &str,
        etag: Option<&str>,
    ) -> Result<Fetched, ApiError> {
        self.get_bytes(&format!("files/{file_id}/preview"), etag)
            .await
    }

    /// `GET files/{file_id}/info`.
    pub async fn file_info(&self, file_id: &str) -> Result<FileInfo, ApiError> {
        self.get(&format!("files/{file_id}/info")).await
    }

    /// `GET files/{file_id}/link`: a public link, when the server allows them.
    pub async fn file_link(&self, file_id: &str) -> Result<String, ApiError> {
        self.get::<PublicLink>(&format!("files/{file_id}/link"))
            .await
            .map(|public| public.link)
    }

    /// `GET emoji/{emoji_id}/image`: a custom emoji's image.
    pub async fn emoji_image(
        &self,
        emoji_id: &str,
        etag: Option<&str>,
    ) -> Result<Fetched, ApiError> {
        self.get_bytes(&format!("emoji/{emoji_id}/image"), etag)
            .await
    }

    /// `GET image?url=`: an outside image through the server's proxy. When
    /// the server's proxy is off it redirects to the image, which is
    /// [`ApiError::Redirected`] with the image's own URL.
    pub async fn proxied_image(&self, url: &str) -> Result<Fetched, ApiError> {
        self.get_bytes_query("image", &[("url", url.to_owned())], None)
            .await
    }
}
