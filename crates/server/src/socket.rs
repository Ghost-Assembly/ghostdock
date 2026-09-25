//! What every WebSocket that streams to the browser does alike: it closes
//! when the credential that opened it is revoked, pings through quiet
//! spells, and ends when the client goes.

use std::future::Future;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures::{SinkExt as _, StreamExt as _};

/// How often a socket is pinged. Proxies commonly drop a connection that
/// stays quiet for a minute, and a deploy or a shell can be quiet for far
/// longer than that.
pub const PING: Duration = Duration::from_secs(20);

/// What a socket carries.
pub trait Source: Send {
    /// The next message to send.
    fn next(&mut self) -> impl Future<Output = Next> + Send;

    /// A text message from the client. Ignored unless a source listens.
    fn heard(&mut self, _text: &str) {}
}

pub enum Next {
    /// Send this and carry on.
    Send(Message),
    /// There is nothing more: send this, if anything, and stop.
    End(Option<Message>),
}

/// A close frame carrying `reason`.
#[must_use]
pub fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

/// Runs a socket until its source ends, the client leaves, or `revoked`
/// completes, which closes it with the reason `revoked`.
pub async fn pump(
    socket: WebSocket,
    mut source: impl Source,
    revoked: impl Future<Output = ()> + Send,
) {
    let (mut sink, mut incoming) = socket.split();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
    tokio::pin!(revoked);
    loop {
        tokio::select! {
            () = &mut revoked => {
                let _ = sink.send(close(close_code::POLICY, "revoked")).await;
                return;
            }
            next = source.next() => match next {
                Next::Send(message) => {
                    if sink.send(message).await.is_err() {
                        return;
                    }
                }
                Next::End(last) => {
                    if let Some(last) = last {
                        let _ = sink.send(last).await;
                    }
                    return;
                }
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => source.heard(&text),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}
