//! Wayland Input Method implementation
//!
//! Implements the `zwp_input_method_v2` protocol from wayland-protocols-misc
//! for input method functionality on Wayland compositors.
//!
//! Also provides the `zwp_text_input_v3` implementation for applications
//! that want to receive text input from an IME.

mod input_method;
mod text_input;

pub use input_method::InputMethod;
pub use text_input::{TextInput, TextInputEvent};
