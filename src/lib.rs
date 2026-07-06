//! # imekit
//!
//! A cross-platform Rust library for IME (Input Method Engine) integration using native protocols.
//!
//! This crate provides native protocol implementations for:
//! - **Linux/Wayland**: `zwp_input_method_v2` and `zwp_text_input_v3` protocols
//! - **Linux/X11**: XIM (X Input Method) protocol
//! - **Linux/IBus**: IBus D-Bus interface (fallback when Wayland protocol is unavailable)
//! - **Windows**: Text Services Framework (TSF)
//! - **macOS**: Input Method Kit (IMK)
//!
//! ## Protocol Support
//!
//! ### Linux - Wayland
//! Uses `wayland-protocols-misc` for the input-method-unstable-v2 protocol which provides:
//! - Input method registration and lifecycle
//! - Text commit and preedit handling
//! - Surrounding text context
//! - Popup surface creation for candidate windows
//!
//! ### Linux - X11
//! Uses XIM (X Input Method) protocol for:
//! - Full XIM server implementation
//! - Text commit via XIM protocol or XTest extension
//! - Preedit handling
//!
//! ### Linux - IBus
//! Uses IBus D-Bus interface (enabled with `ibus` feature) as a fallback:
//! - Works when Wayland input-method protocol is not available
//! - Provides text commit functionality via IBus
//!
//! ### Windows
//! Uses the Text Services Framework (TSF) for:
//! - Input processor registration
//! - Text composition via SendInput
//! - Candidate window management
//!
//! ### macOS
//! Uses the Input Method Kit (IMK) framework for:
//! - Native NSTextInputClient integration
//! - Text input handling via CGEvent
//!
//! ## Features
//!
//! - `log` - Enable logging via the `log` crate
//! - `tracing` - Enable logging via the `tracing` crate
//! - `ibus` - Enable IBus support for Linux (requires `zbus`)
//!
//! ## Example
//!
//! ```rust,no_run
//! use imekit::{InputMethod, InputMethodEvent};
//!
//! // Create an input method instance
//! let mut im = InputMethod::new()?;
//!
//! // Handle events
//! while let Some(event) = im.next_event() {
//!     match event {
//!         InputMethodEvent::Activate { serial } => {
//!             // IME activated - ready to commit text
//!             im.commit_string("Hello!")?;
//!             im.commit(serial)?;
//!         }
//!         InputMethodEvent::Deactivate => {
//!             // IME deactivated
//!         }
//!         InputMethodEvent::SurroundingText { text, cursor, anchor } => {
//!             // Got surrounding text context
//!         }
//!         _ => {}
//!     }
//! }
//! # Ok::<(), imekit::Error>(())
//! ```

#[macro_use]
mod logging;
mod error;
mod types;

#[cfg(target_os = "linux")]
mod linux_xtest;

#[cfg(target_os = "linux")]
mod wayland;

#[cfg(target_os = "linux")]
mod x11;

#[cfg(all(target_os = "linux", feature = "ibus"))]
mod ibus;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

pub use error::{Error, Result};
pub use types::*;

/// The display server being used on Linux
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    /// Wayland display server
    Wayland,
    /// X11 display server
    X11,
}

#[cfg(target_os = "linux")]
impl DisplayServer {
    /// Detect the current display server
    pub fn detect() -> Option<Self> {
        // Check for Wayland first
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            Some(DisplayServer::Wayland)
        } else if std::env::var("DISPLAY").is_ok() {
            Some(DisplayServer::X11)
        } else {
            None
        }
    }
}

// Re-export Wayland-specific types (always available on Linux for building)
#[cfg(target_os = "linux")]
pub mod wayland_impl {
    pub use super::wayland::{InputMethod as WaylandInputMethod, TextInput, TextInputEvent};
}

// Re-export X11-specific types
#[cfg(target_os = "linux")]
pub mod x11_impl {
    pub use super::x11::InputMethod as X11InputMethod;
}

// Re-export IBus-specific types (when feature is enabled)
#[cfg(all(target_os = "linux", feature = "ibus"))]
pub mod ibus_impl {
    pub use super::ibus::InputMethod as IBusInputMethod;
}

// Main InputMethod that auto-detects display server
#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::InputMethod;

#[cfg(target_os = "windows")]
pub use windows::InputMethod;

#[cfg(target_os = "macos")]
pub use macos::InputMethod;
