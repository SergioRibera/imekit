//! IBus Input Method implementation using zbus
//!
//! This module implements IME functionality using the IBus D-Bus interface.
//! IBus is a common input method framework on Linux that provides a D-Bus API
//! for text input.
//!
//! This implementation serves as a fallback when the Wayland input-method protocol
//! is not available (e.g., on some compositors or desktop environments).
//!
//! ## Limitations
//!
//! - **Event handling**: IBus uses D-Bus signals for events which require async handling.
//!   Currently, `next_event()` only returns events queued during operations like `new()`.
//! - **Preedit**: Preedit (composition) support is limited as it requires D-Bus signal handling.
//!
//! ## IBus D-Bus interface
//!
//! - `org.freedesktop.IBus` - Main service
//! - `org.freedesktop.IBus.InputContext` - Input context for text input

use std::collections::VecDeque;

use zbus::blocking::Connection;

use crate::{Error, InputMethodEvent, InputMethodState, Result};

/// IBus D-Bus service name
const IBUS_SERVICE: &str = "org.freedesktop.IBus";
/// IBus D-Bus object path
const IBUS_PATH: &str = "/org/freedesktop/IBus";
/// IBus D-Bus interface
const IBUS_INTERFACE: &str = "org.freedesktop.IBus";
/// IBus input context interface
const IBUS_INPUT_CONTEXT_INTERFACE: &str = "org.freedesktop.IBus.InputContext";

/// IBus input method implementation using D-Bus
pub struct InputMethod {
    /// D-Bus connection
    connection: Connection,
    /// Input context object path
    input_context_path: Option<String>,
    /// Current state
    state: InputMethodState,
    /// Pending events
    events: VecDeque<InputMethodEvent>,
    /// Serial number
    serial: u32,
}

impl InputMethod {
    /// Create a new IBus input method instance
    ///
    /// This connects to the IBus D-Bus service and creates an input context.
    pub fn new() -> Result<Self> {
        // Connect to the session bus
        let connection = Connection::session()
            .map_err(|e| Error::IBus(format!("Failed to connect to D-Bus session bus: {}", e)))?;

        // Check if IBus service is available
        let proxy = connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(IBUS_SERVICE,),
        );

        let has_owner: bool = match proxy {
            Ok(reply) => reply
                .body()
                .deserialize()
                .map_err(|e| Error::IBus(format!("Failed to check IBus service: {}", e)))?,
            Err(e) => {
                return Err(Error::IBus(format!("Failed to query D-Bus: {}", e)));
            }
        };

        if !has_owner {
            return Err(Error::IBus("IBus service is not running".to_string()));
        }

        let mut im = Self {
            connection,
            input_context_path: None,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            serial: 0,
        };

        // Try to create an input context
        im.create_input_context()?;

        Ok(im)
    }

    /// Create an input context via IBus
    fn create_input_context(&mut self) -> Result<()> {
        // Call CreateInputContext on the IBus service
        let reply = self
            .connection
            .call_method(
                Some(IBUS_SERVICE),
                IBUS_PATH,
                Some(IBUS_INTERFACE),
                "CreateInputContext",
                &("imekit",),
            )
            .map_err(|e| Error::IBus(format!("Failed to create input context: {}", e)))?;

        let context_path: String = reply
            .body()
            .deserialize()
            .map_err(|e| Error::IBus(format!("Failed to parse input context path: {}", e)))?;

        log_debug!("Created IBus input context: {}", context_path);
        self.input_context_path = Some(context_path);

        // Activate the input context
        self.focus_in()?;

        Ok(())
    }

    /// Focus in to activate the input context
    fn focus_in(&mut self) -> Result<()> {
        if let Some(ref path) = self.input_context_path {
            self.connection
                .call_method(
                    Some(IBUS_SERVICE),
                    path.as_str(),
                    Some(IBUS_INPUT_CONTEXT_INTERFACE),
                    "FocusIn",
                    &(),
                )
                .map_err(|e| Error::IBus(format!("Failed to focus in: {}", e)))?;

            self.state.active = true;
            self.serial += 1;
            self.state.serial = self.serial;
            self.events.push_back(InputMethodEvent::Activate {
                serial: self.serial,
            });
        }
        Ok(())
    }

    /// Focus out to deactivate the input context
    fn focus_out(&mut self) -> Result<()> {
        if let Some(ref path) = self.input_context_path {
            self.connection
                .call_method(
                    Some(IBUS_SERVICE),
                    path.as_str(),
                    Some(IBUS_INPUT_CONTEXT_INTERFACE),
                    "FocusOut",
                    &(),
                )
                .map_err(|e| Error::IBus(format!("Failed to focus out: {}", e)))?;

            self.state.active = false;
            self.events.push_back(InputMethodEvent::Deactivate);
        }
        Ok(())
    }

    /// Get the next event
    ///
    /// Returns the next pending input method event if available.
    ///
    /// **Note:** IBus uses D-Bus signals for events which would require async handling.
    /// Currently, this method only returns events that were queued during other operations
    /// (like `new()` or `focus_in()`). For full event support, consider using the
    /// async IBus D-Bus signal handling.
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        // Return any queued events first
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }

        // IBus uses signals for events, which would require async handling
        // For now, we return None if no events are queued
        None
    }

    /// Check if the input method is active
    pub fn is_active(&self) -> bool {
        self.state.active
    }

    /// Commit text via IBus
    ///
    /// This commits text by simulating key events through IBus's ProcessKeyEvent method.
    /// This approach is more reliable than trying to construct IBusText D-Bus structures
    /// directly.
    pub fn commit_string(&self, text: &str) -> Result<()> {
        if self.input_context_path.is_some() {
            self.commit_string_via_keys(text)
        } else {
            Err(Error::NotActive)
        }
    }

    /// Commit text by simulating key events
    fn commit_string_via_keys(&self, text: &str) -> Result<()> {
        if let Some(ref path) = self.input_context_path {
            for c in text.chars() {
                let keyval = char_to_ibus_keyval(c);
                if keyval != 0 {
                    // Process key press
                    let _: bool = self
                        .connection
                        .call_method(
                            Some(IBUS_SERVICE),
                            path.as_str(),
                            Some(IBUS_INPUT_CONTEXT_INTERFACE),
                            "ProcessKeyEvent",
                            &(keyval, 0u32, 0u32), // keyval, keycode, state
                        )
                        .map_err(|e| Error::IBus(format!("ProcessKeyEvent failed: {}", e)))?
                        .body()
                        .deserialize()
                        .map_err(|e| Error::IBus(format!("Failed to parse response: {}", e)))?;

                    // Process key release
                    let _: bool = self
                        .connection
                        .call_method(
                            Some(IBUS_SERVICE),
                            path.as_str(),
                            Some(IBUS_INPUT_CONTEXT_INTERFACE),
                            "ProcessKeyEvent",
                            &(keyval, 0u32, 0x80000000u32), // IBUS_RELEASE_MASK
                        )
                        .map_err(|e| Error::IBus(format!("ProcessKeyEvent release failed: {}", e)))?
                        .body()
                        .deserialize()
                        .map_err(|e| Error::IBus(format!("Failed to parse response: {}", e)))?;
                }
            }
            Ok(())
        } else {
            Err(Error::NotActive)
        }
    }

    /// Set preedit string
    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, cursor_end: i32) -> Result<()> {
        if let Some(ref _path) = self.input_context_path {
            // IBus preedit is handled via UpdatePreeditText signal
            // This would require the application to listen for signals
            log_debug!(
                "IBus preedit: {} (cursor: {}-{})",
                text,
                cursor_begin,
                cursor_end
            );
            Ok(())
        } else {
            Err(Error::NotActive)
        }
    }

    /// Delete surrounding text
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        if let Some(ref path) = self.input_context_path {
            // IBus DeleteSurroundingText: offset (from cursor), nchars
            // Delete before: offset = -(before as i32), nchars = before
            if before > 0 {
                self.connection
                    .call_method(
                        Some(IBUS_SERVICE),
                        path.as_str(),
                        Some(IBUS_INPUT_CONTEXT_INTERFACE),
                        "DeleteSurroundingText",
                        &(-(before as i32), before),
                    )
                    .map_err(|e| {
                        Error::IBus(format!("DeleteSurroundingText (before) failed: {}", e))
                    })?;
            }

            // Delete after: offset = 0, nchars = after
            if after > 0 {
                self.connection
                    .call_method(
                        Some(IBUS_SERVICE),
                        path.as_str(),
                        Some(IBUS_INPUT_CONTEXT_INTERFACE),
                        "DeleteSurroundingText",
                        &(0i32, after),
                    )
                    .map_err(|e| {
                        Error::IBus(format!("DeleteSurroundingText (after) failed: {}", e))
                    })?;
            }

            Ok(())
        } else {
            Err(Error::NotActive)
        }
    }

    /// Commit changes (finalize)
    pub fn commit(&self, _serial: u32) -> Result<()> {
        // IBus commits are immediate
        Ok(())
    }

    /// Get the current state
    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}

impl Drop for InputMethod {
    fn drop(&mut self) {
        // Clean up: focus out and potentially destroy the input context
        let _ = self.focus_out();
    }
}

/// Convert a character to IBus keyval
///
/// IBus keyvals are based on X11 keysyms
fn char_to_ibus_keyval(c: char) -> u32 {
    let code = c as u32;

    // ASCII printable characters (0x20-0x7E) map directly
    if (0x20..=0x7E).contains(&code) {
        return code;
    }

    // Extended Latin (Latin-1) characters
    if (0xA0..=0xFF).contains(&code) {
        return code;
    }

    // Unicode characters above U+00FF use the Unicode keysym format
    // Format: 0x01000000 | unicode_codepoint
    if code > 0xFF {
        return 0x0100_0000 | code;
    }

    // Unknown character
    0
}
