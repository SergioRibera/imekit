//! X11 Input Method implementation using XIM
//!
//! This module implements IME functionality using the X Input Method (XIM) protocol
//! and the XTest extension for text input simulation.

use std::collections::VecDeque;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use crate::{linux_xtest, Error, InputMethodEvent, InputMethodState, Result};

/// X11 input method implementation using XIM
pub struct InputMethod {
    connection: RustConnection,
    #[allow(dead_code)]
    screen_num: usize,
    root_window: Window,
    serial: u32,
    keyboard_mapping: Option<linux_xtest::CachedKeyboardMapping>,
    events: VecDeque<InputMethodEvent>,
    state: InputMethodState,
    focused_window: Option<Window>,
}

impl InputMethod {
    /// Create a new input method instance
    pub fn new() -> Result<Self> {
        let (connection, screen_num) =
            RustConnection::connect(None).map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let setup = connection.setup();
        let screen = &setup.roots[screen_num];
        let root_window = screen.root;

        let xtest_ext = connection
            .query_extension(b"XTEST")
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        if !xtest_ext.present {
            return Err(Error::ProtocolNotSupported(
                "XTest extension not available".to_string(),
            ));
        }

        x11rb::protocol::xtest::get_version(&connection, 2, 2)
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let event_mask = EventMask::FOCUS_CHANGE | EventMask::PROPERTY_CHANGE;
        connection
            .change_window_attributes(
                root_window,
                &ChangeWindowAttributesAux::new().event_mask(event_mask),
            )
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        connection
            .flush()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let mut im = Self {
            connection,
            screen_num,
            root_window,
            serial: 0,
            keyboard_mapping: None,
            events: VecDeque::new(),
            state: InputMethodState::new(),
            focused_window: None,
        };

        im.refresh_keyboard_mapping()?;
        im.check_focus()?;

        Ok(im)
    }

    fn refresh_keyboard_mapping(&mut self) -> Result<()> {
        self.keyboard_mapping = Some(linux_xtest::load_keyboard_mapping(&self.connection)?);
        Ok(())
    }

    fn check_focus(&mut self) -> Result<()> {
        let focus = self
            .connection
            .get_input_focus()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        // 0 = None, 1 = PointerRoot (input follows pointer, not a real window)
        let new_focus = if focus.focus > 1 { Some(focus.focus) } else { None };

        if new_focus != self.focused_window {
            self.focused_window = new_focus;

            if new_focus.is_some() && !self.state.active {
                self.serial += 1;
                self.state.active = true;
                self.state.serial = self.serial;
                self.events.push_back(InputMethodEvent::Activate { serial: self.serial });
            } else if new_focus.is_none() && self.state.active {
                self.state.active = false;
                self.events.push_back(InputMethodEvent::Deactivate);
            }
        }

        Ok(())
    }

    /// Get the next event
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }

        loop {
            match self.connection.poll_for_event() {
                Ok(Some(event)) => {
                    if let Some(ime_event) = self.process_x11_event(&event) {
                        return Some(ime_event);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    log_warn!("X11 event error: {}", e);
                    break;
                }
            }
        }

        self.events.pop_front()
    }

    fn process_x11_event(&mut self, event: &x11rb::protocol::Event) -> Option<InputMethodEvent> {
        match event {
            x11rb::protocol::Event::FocusIn(focus_in) => {
                log_debug!("FocusIn event for window: {:?}", focus_in.event);
                self.focused_window = Some(focus_in.event);
                if !self.state.active {
                    self.serial += 1;
                    self.state.active = true;
                    self.state.serial = self.serial;
                    return Some(InputMethodEvent::Activate { serial: self.serial });
                }
            }
            x11rb::protocol::Event::FocusOut(focus_out) => {
                log_debug!("FocusOut event for window: {:?}", focus_out.event);
                if self.state.active {
                    self.state.active = false;
                    return Some(InputMethodEvent::Deactivate);
                }
            }
            x11rb::protocol::Event::KeyPress(key_press) => {
                log_debug!(
                    "KeyPress event: keycode={}, state={:?}",
                    key_press.detail,
                    key_press.state
                );
            }
            x11rb::protocol::Event::PropertyNotify(prop_notify) => {
                log_debug!(
                    "PropertyNotify: atom={}, state={:?}",
                    prop_notify.atom,
                    prop_notify.state
                );
            }
            x11rb::protocol::Event::MappingNotify(_) => {
                log_debug!("MappingNotify - refreshing keyboard mapping");
                if let Err(e) = self.refresh_keyboard_mapping() {
                    log_warn!("Failed to refresh keyboard mapping: {}", e);
                }
            }
            _ => {}
        }
        None
    }

    /// Dispatch events (blocking)
    pub fn dispatch(&mut self) -> Result<()> {
        match self.connection.wait_for_event() {
            Ok(event) => {
                if let Some(ime_event) = self.process_x11_event(&event) {
                    self.events.push_back(ime_event);
                }
                Ok(())
            }
            Err(e) => Err(Error::ConnectionFailed(e.to_string())),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.active
    }

    /// Commit text to the focused window using XTest extension
    pub fn commit_string(&self, text: &str) -> Result<()> {
        let mapping = self
            .keyboard_mapping
            .as_ref()
            .ok_or_else(|| Error::ConnectionFailed("No keyboard mapping".to_string()))?;
        linux_xtest::commit_string(&self.connection, self.root_window, mapping, text)
    }

    pub fn set_preedit_string(
        &self,
        _text: &str,
        _cursor_begin: i32,
        _cursor_end: i32,
    ) -> Result<()> {
        log_debug!("Preedit not supported in X11 XTest mode");
        Ok(())
    }

    /// Delete surrounding text by sending backspace/delete keys
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        let mapping = self
            .keyboard_mapping
            .as_ref()
            .ok_or_else(|| Error::ConnectionFailed("No keyboard mapping".to_string()))?;
        linux_xtest::delete_surrounding_text(&self.connection, self.root_window, mapping, before, after)
    }

    /// Commit changes (no-op for X11 as commits are immediate)
    pub fn commit(&self, _serial: u32) -> Result<()> {
        Ok(())
    }

    pub fn grab_keyboard(&mut self) -> Result<()> {
        Err(Error::ProtocolNotSupported(
            "Keyboard grab not supported in X11 XTest mode".to_string(),
        ))
    }

    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}
