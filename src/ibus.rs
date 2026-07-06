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
//! - **Text commit**: Uses XTest for text injection since IBus commit requires engine registration.
//!
//! ## IBus D-Bus interface
//!
//! - `org.freedesktop.IBus` - Main service
//! - `org.freedesktop.IBus.InputContext` - Input context for text input

use std::collections::VecDeque;

use zbus::blocking::Connection;

use crate::{linux_xtest, Error, InputMethodEvent, InputMethodState, Result};

const IBUS_SERVICE: &str = "org.freedesktop.IBus";
const IBUS_PATH: &str = "/org/freedesktop/IBus";
const IBUS_INTERFACE: &str = "org.freedesktop.IBus";
const IBUS_INPUT_CONTEXT_INTERFACE: &str = "org.freedesktop.IBus.InputContext";

/// IBus input method implementation using D-Bus
pub struct InputMethod {
    connection: Connection,
    input_context_path: Option<String>,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
    serial: u32,
    xtest: Option<linux_xtest::XTestWriter>,
}

impl InputMethod {
    pub fn new() -> Result<Self> {
        let connection = Connection::session()
            .map_err(|e| Error::IBus(format!("Failed to connect to D-Bus session bus: {}", e)))?;

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
            Err(e) => return Err(Error::IBus(format!("Failed to query D-Bus: {}", e))),
        };

        if !has_owner {
            return Err(Error::IBus("IBus service is not running".to_string()));
        }

        let xtest = linux_xtest::XTestWriter::new().ok();

        let mut im = Self {
            connection,
            input_context_path: None,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            serial: 0,
            xtest,
        };

        im.create_input_context()?;

        Ok(im)
    }

    fn create_input_context(&mut self) -> Result<()> {
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

        self.focus_in()?;

        Ok(())
    }

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
            self.events.push_back(InputMethodEvent::Activate { serial: self.serial });
        }
        Ok(())
    }

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

    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        self.events.pop_front()
    }

    pub fn is_active(&self) -> bool {
        self.state.active
    }

    /// Commit text via XTest (IBus commit requires engine registration; XTest is the
    /// reliable fallback for X11/XWayland sessions where IBus is typically deployed).
    pub fn commit_string(&self, text: &str) -> Result<()> {
        self.xtest
            .as_ref()
            .ok_or_else(|| {
                Error::ProtocolNotSupported(
                    "XTest not available; IBus text commit requires an X11 display".to_string(),
                )
            })?
            .commit_string(text)
    }

    pub fn set_preedit_string(&self, _text: &str, _cursor_begin: i32, _cursor_end: i32) -> Result<()> {
        log_debug!("Preedit not supported in IBus fallback mode");
        Ok(())
    }

    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        self.xtest
            .as_ref()
            .ok_or_else(|| {
                Error::ProtocolNotSupported(
                    "XTest not available; IBus text deletion requires an X11 display".to_string(),
                )
            })?
            .delete_surrounding_text(before, after)
    }

    pub fn commit(&self, _serial: u32) -> Result<()> {
        Ok(())
    }

    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}

impl Drop for InputMethod {
    fn drop(&mut self) {
        _ = self.focus_out();
    }
}
