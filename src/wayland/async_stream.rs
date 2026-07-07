//! Async stream wrapper for the Wayland input method.
//!
//! Enabled with the `async` cargo feature. Works with any executor that
//! drives `futures-core::Stream` (tokio, async-std, smol, etc.).

use std::os::unix::io::AsFd;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::InputMethod;
use crate::InputMethodEvent;

/// An async [`futures_core::Stream`] of [`InputMethodEvent`]s.
///
/// Created via [`InputMethod::into_stream`]. Uses the Wayland socket fd as
/// a readability signal so the executor is woken only when events arrive.
pub struct InputMethodStream {
    im: InputMethod,
    io: async_io::Async<std::os::fd::OwnedFd>,
}

impl InputMethodStream {
    pub(super) fn new(im: InputMethod) -> std::io::Result<Self> {
        // Dup the socket fd — we register the dup in the reactor so setting
        // its flags (if any) doesn't affect the original connection fd.
        let fd = im.connection.as_fd().try_clone_to_owned()?;
        // new_nonblocking: register for readability polling without forcing
        // O_NONBLOCK; we delegate actual reads to wayland-client.
        let io = async_io::Async::new_nonblocking(fd)?;
        Ok(Self { im, io })
    }
}

impl futures_core::Stream for InputMethodStream {
    type Item = InputMethodEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.im.next_event() {
                return Poll::Ready(Some(event));
            }
            match self.io.poll_readable(cx) {
                Poll::Ready(Ok(_)) => continue,
                Poll::Ready(Err(_)) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
