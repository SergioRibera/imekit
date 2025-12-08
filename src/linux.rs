//! Linux input method implementation
//!
//! This module provides the unified InputMethod type that auto-detects
//! Wayland vs X11 and delegates to the appropriate backend.
//!
//! When the `ibus` feature is enabled, IBus support is available as a fallback
//! when the Wayland input-method protocol is not available.

use crate::{wayland, x11, DisplayServer, Error, InputMethodEvent, InputMethodState, Result};

#[cfg(feature = "ibus")]
use crate::ibus;

/// Linux input method that auto-detects display server
#[allow(clippy::large_enum_variant)]
pub enum InputMethod {
    /// Wayland input method
    Wayland(wayland::InputMethod),
    /// X11 input method
    X11(x11::InputMethod),
    /// IBus input method (fallback when Wayland protocol unavailable)
    #[cfg(feature = "ibus")]
    IBus(ibus::InputMethod),
}

macro_rules! ibus_fallback {
    ($n:ident : $i:ident) => {
        match $n::InputMethod::new() {
            Ok(im) => Ok(InputMethod::$i(im)),
            #[cfg(feature = "ibus")]
            Err(Error::ProtocolNotSupported(_)) => {
                // Fallback to IBus if Wayland protocol is not supported
                log_info!("Wayland input-method protocol not available, falling back to IBus");
                let im = ibus::InputMethod::new()?;
                Ok(InputMethod::IBus(im))
            }
            Err(e) => Err(e),
        }
    };
}

impl InputMethod {
    /// Create a new input method, auto-detecting the display server
    ///
    /// On Wayland, this will first try to use the native `zwp_input_method_v2` protocol.
    /// If that fails and the `ibus` feature is enabled, it will fall back to IBus.
    pub fn new() -> Result<Self> {
        match DisplayServer::detect() {
            Some(DisplayServer::Wayland) => {
                // Try Wayland native protocol first
                ibus_fallback!(wayland : Wayland)
            }
            Some(DisplayServer::X11) => {
                ibus_fallback!(x11 : X11)
            }
            None => Err(Error::ConnectionFailed(
                "No display server detected (set WAYLAND_DISPLAY or DISPLAY)".to_string(),
            )),
        }
    }

    /// Create a Wayland input method explicitly
    pub fn wayland() -> Result<Self> {
        let im = wayland::InputMethod::new()?;
        Ok(InputMethod::Wayland(im))
    }

    /// Create an X11 input method explicitly
    pub fn x11() -> Result<Self> {
        let im = x11::InputMethod::new()?;
        Ok(InputMethod::X11(im))
    }

    /// Create an IBus input method explicitly
    #[cfg(feature = "ibus")]
    pub fn ibus() -> Result<Self> {
        let im = ibus::InputMethod::new()?;
        Ok(InputMethod::IBus(im))
    }

    /// Get the next event
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        match self {
            InputMethod::Wayland(im) => im.next_event(),
            InputMethod::X11(im) => im.next_event(),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.next_event(),
        }
    }

    /// Check if active
    pub fn is_active(&self) -> bool {
        match self {
            InputMethod::Wayland(im) => im.is_active(),
            InputMethod::X11(im) => im.is_active(),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.is_active(),
        }
    }

    /// Commit text
    pub fn commit_string(&self, text: &str) -> Result<()> {
        match self {
            InputMethod::Wayland(im) => im.commit_string(text),
            InputMethod::X11(im) => im.commit_string(text),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.commit_string(text),
        }
    }

    /// Set preedit string
    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, cursor_end: i32) -> Result<()> {
        match self {
            InputMethod::Wayland(im) => im.set_preedit_string(text, cursor_begin, cursor_end),
            InputMethod::X11(im) => im.set_preedit_string(text, cursor_begin, cursor_end),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.set_preedit_string(text, cursor_begin, cursor_end),
        }
    }

    /// Delete surrounding text
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        match self {
            InputMethod::Wayland(im) => im.delete_surrounding_text(before, after),
            InputMethod::X11(im) => im.delete_surrounding_text(before, after),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.delete_surrounding_text(before, after),
        }
    }

    /// Commit changes
    pub fn commit(&self, serial: u32) -> Result<()> {
        match self {
            InputMethod::Wayland(im) => im.commit(serial),
            InputMethod::X11(im) => im.commit(serial),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.commit(serial),
        }
    }

    /// Check if this is a Wayland input method
    pub fn is_wayland(&self) -> bool {
        matches!(self, InputMethod::Wayland(_))
    }

    /// Check if this is an X11 input method
    pub fn is_x11(&self) -> bool {
        matches!(self, InputMethod::X11(_))
    }

    /// Check if this is an IBus input method
    #[cfg(feature = "ibus")]
    pub fn is_ibus(&self) -> bool {
        matches!(self, InputMethod::IBus(_))
    }

    /// Get the current state
    pub fn state(&self) -> InputMethodState {
        match self {
            InputMethod::Wayland(im) => im.state(),
            InputMethod::X11(im) => im.state(),
            #[cfg(feature = "ibus")]
            InputMethod::IBus(im) => im.state(),
        }
    }
}
