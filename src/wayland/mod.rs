//! Wayland Input Method implementation
//!
//! Implements the `zwp_input_method_v2` protocol from wayland-protocols-misc
//! for input method functionality on Wayland compositors.
//!
//! Also provides the `zwp_text_input_v3` implementation for applications
//! that want to receive text input from an IME.

mod input_method;
mod text_input;

#[cfg(feature = "async")]
mod async_stream;

pub use input_method::{InputMethod, InputMethodHandle, PopupSurface};
pub use text_input::{TextInput, TextInputEvent};

#[cfg(feature = "async")]
pub use async_stream::InputMethodStream;
