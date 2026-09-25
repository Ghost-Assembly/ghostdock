//! A WebSocket that belongs to whoever holds it.
//!
//! A browser keeps a socket open, and keeps calling its handlers, until it
//! is closed explicitly: dropping the Rust handle does neither. A screen
//! that opened one and forgot it would leave a shell running on the server
//! and closures writing to signals nothing renders. Holding an [`Owned`]
//! instead ties the connection to its holder: dropping it detaches the
//! handlers, closes the socket and frees the callbacks, without reporting
//! the close as an ending, since whoever dropped it already knows.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

/// The `ws://` or `wss://` address of `path` on this page's own host.
#[must_use]
pub fn url(path: &str) -> String {
    let location = web_sys::window().map(|w| w.location());
    let host = location
        .as_ref()
        .and_then(|l| l.host().ok())
        .unwrap_or_default();
    let secure = location
        .as_ref()
        .and_then(|l| l.protocol().ok())
        .is_some_and(|p| p == "https:");
    format!("{}://{host}{path}", if secure { "wss" } else { "ws" })
}

/// An open socket and the callbacks the browser holds for it.
pub struct Owned {
    socket: WebSocket,
    _message: Option<Closure<dyn FnMut(MessageEvent)>>,
    _open: Option<Closure<dyn FnMut()>>,
    _close: Option<Closure<dyn FnMut(CloseEvent)>>,
}

impl Owned {
    /// Starts connecting to `url`. Handlers are attached before the page
    /// yields, so none of the socket's events can be missed.
    pub fn connect(url: &str) -> Result<Self, wasm_bindgen::JsValue> {
        Ok(Self {
            socket: WebSocket::new(url)?,
            _message: None,
            _open: None,
            _close: None,
        })
    }

    #[must_use]
    pub fn on_message(mut self, f: impl FnMut(MessageEvent) + 'static) -> Self {
        let f = Closure::<dyn FnMut(MessageEvent)>::new(f);
        self.socket.set_onmessage(Some(f.as_ref().unchecked_ref()));
        self._message = Some(f);
        self
    }

    #[must_use]
    pub fn on_open(mut self, f: impl FnMut() + 'static) -> Self {
        let f = Closure::<dyn FnMut()>::new(f);
        self.socket.set_onopen(Some(f.as_ref().unchecked_ref()));
        self._open = Some(f);
        self
    }

    /// Called when the other end, or the network, ends the connection.
    /// Not called when this is dropped.
    #[must_use]
    pub fn on_close(mut self, f: impl FnMut(CloseEvent) + 'static) -> Self {
        let f = Closure::<dyn FnMut(CloseEvent)>::new(f);
        self.socket.set_onclose(Some(f.as_ref().unchecked_ref()));
        self._close = Some(f);
        self
    }

    /// Sends `text`; false when the socket is not open to take it.
    pub fn send(&self, text: &str) -> bool {
        // Checked first: a browser logs an error for every send to a socket
        // that has closed, and one that is still connecting throws.
        self.socket.ready_state() == WebSocket::OPEN && self.socket.send_with_str(text).is_ok()
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // Detached first: the browser must not call a closure freed below.
        self.socket.set_onmessage(None);
        self.socket.set_onopen(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}
