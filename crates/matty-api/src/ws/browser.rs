//! The browser's `WebSocket`, seen as tungstenite frames so the protocol
//! code in `ws` runs unchanged on the web.
//!
//! The browser answers pings and does the closing handshake itself, so only
//! text and binary frames cross this boundary, plus the close the server
//! sent, which arrives as [`Message::Close`] like on the desktop.

use std::pin::Pin;

use futures_util::{Sink, SinkExt, StreamExt, future, stream};
use gloo_net::websocket::{Message as BrowserMessage, WebSocketError, futures::WebSocket};
use tokio_tungstenite::tungstenite::{
    self, Message,
    protocol::{CloseFrame, frame::coding::CloseCode},
};

use super::Socket;
use crate::ApiError;

/// Opens `url` and resolves once the browser says the socket is open.
pub(super) async fn open(url: &url::Url) -> Result<impl Socket, ApiError> {
    let mut socket =
        WebSocket::open(url.as_str()).map_err(|error| ApiError::InvalidUrl(error.to_string()))?;
    // `poll_ready` waits for the open event, or fails if it never comes.
    future::poll_fn(|cx| Sink::<BrowserMessage>::poll_ready(Pin::new(&mut socket), cx))
        .await
        .map_err(|error| ApiError::Network(error.to_string()))?;
    Ok(socket
        .sink_map_err(io_error)
        .with_flat_map(|frame: Message| stream::iter(outgoing(frame).map(Ok)))
        .map(incoming))
}

/// A frame for the browser to send, or nothing for the control frames it
/// handles itself.
fn outgoing(frame: Message) -> Option<BrowserMessage> {
    match frame {
        Message::Text(text) => Some(BrowserMessage::Text(text.to_string())),
        Message::Binary(bytes) => Some(BrowserMessage::Bytes(bytes.to_vec())),
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => None,
    }
}

/// The browser's next event as a tungstenite frame. The close event
/// becomes [`Message::Close`] with its code and reason.
fn incoming(event: Result<BrowserMessage, WebSocketError>) -> Result<Message, tungstenite::Error> {
    match event {
        Ok(BrowserMessage::Text(text)) => Ok(Message::text(text)),
        Ok(BrowserMessage::Bytes(bytes)) => Ok(Message::binary(bytes)),
        Err(WebSocketError::ConnectionClose(close)) => Ok(Message::Close(Some(CloseFrame {
            code: CloseCode::from(close.code),
            reason: close.reason.into(),
        }))),
        Err(error) => Err(io_error(error)),
    }
}

fn io_error(error: WebSocketError) -> tungstenite::Error {
    tungstenite::Error::Io(std::io::Error::other(error.to_string()))
}
