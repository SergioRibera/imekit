//! X11 Input Method implementation using XIM
//!
//! This module implements IME functionality using the X Input Method (XIM) protocol
//! and the XTest extension for text input simulation.
//!
//! XIM is the standard input method framework for X11. This implementation uses
//! the XTest extension to simulate key events for text input and monitors X11
//! events for IME state changes.

use std::collections::VecDeque;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::{Error, InputMethodEvent, InputMethodState, Result};

// X11 keysyms for special keys
const XK_SHIFT_L: Keysym = 0xFFE1;
const XK_CONTROL_L: Keysym = 0xFFE3;
const XK_BACKSPACE: Keysym = 0xFF08;
const XK_DELETE: Keysym = 0xFFFF;
const XK_U: Keysym = 0x0075;
const XK_SPACE: Keysym = 0x0020;

/// X11 input method implementation using XIM
pub struct InputMethod {
    connection: RustConnection,
    #[allow(dead_code)]
    screen_num: usize,
    root_window: Window,
    serial: u32,
    /// Cached keyboard mapping
    keyboard_mapping: Option<CachedKeyboardMapping>,
    /// Pending events
    events: VecDeque<InputMethodEvent>,
    /// Current state
    state: InputMethodState,
    /// Focused window
    focused_window: Option<Window>,
}

/// Cached keyboard mapping for performance
struct CachedKeyboardMapping {
    keysyms: Vec<Keysym>,
    keysyms_per_keycode: usize,
    min_keycode: Keycode,
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

        // Initialize XTest
        let _version = xtest::get_version(&connection, 2, 2)
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

        // Cache keyboard mapping
        im.refresh_keyboard_mapping()?;

        // Initial check for focused window
        im.check_focus()?;

        Ok(im)
    }

    /// Refresh the cached keyboard mapping
    fn refresh_keyboard_mapping(&mut self) -> Result<()> {
        let min_keycode = self.connection.setup().min_keycode;
        let max_keycode = self.connection.setup().max_keycode;
        let keycode_count = max_keycode - min_keycode + 1;

        let mapping = self
            .connection
            .get_keyboard_mapping(min_keycode, keycode_count)
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        self.keyboard_mapping = Some(CachedKeyboardMapping {
            keysyms: mapping.keysyms,
            keysyms_per_keycode: mapping.keysyms_per_keycode as usize,
            min_keycode,
        });

        Ok(())
    }

    /// Check the current focus and emit events if changed
    fn check_focus(&mut self) -> Result<()> {
        let focus = self
            .connection
            .get_input_focus()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let new_focus = if focus.focus != 0 {
            Some(focus.focus)
        } else {
            None
        };

        // Check if focus changed
        if new_focus != self.focused_window {
            self.focused_window = new_focus;

            if new_focus.is_some() && !self.state.active {
                // Focus gained - IME becomes active
                self.serial += 1;
                self.state.active = true;
                self.state.serial = self.serial;
                self.events.push_back(InputMethodEvent::Activate {
                    serial: self.serial,
                });
            } else if new_focus.is_none() && self.state.active {
                // Focus lost - IME becomes inactive
                self.state.active = false;
                self.events.push_back(InputMethodEvent::Deactivate);
            }
        }

        Ok(())
    }

    /// Get the next event
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        // First return any queued events
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }

        // Poll for X11 events
        loop {
            match self.connection.poll_for_event() {
                Ok(Some(event)) => {
                    if let Some(ime_event) = self.process_x11_event(&event) {
                        return Some(ime_event);
                    }
                    // Continue polling for more events
                }
                Ok(None) => break, // No more events
                Err(e) => {
                    log_warn!("X11 event error: {}", e);
                    break;
                }
            }
        }

        // Return any events generated during processing
        self.events.pop_front()
    }

    /// Process an X11 event and convert to InputMethodEvent if applicable
    fn process_x11_event(&mut self, event: &x11rb::protocol::Event) -> Option<InputMethodEvent> {
        match event {
            x11rb::protocol::Event::FocusIn(focus_in) => {
                log_debug!("FocusIn event for window: {:?}", focus_in.event);
                self.focused_window = Some(focus_in.event);
                if !self.state.active {
                    self.serial += 1;
                    self.state.active = true;
                    self.state.serial = self.serial;
                    return Some(InputMethodEvent::Activate {
                        serial: self.serial,
                    });
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
                // Key press events might indicate IME input
                log_debug!(
                    "KeyPress event: keycode={}, state={:?}",
                    key_press.detail,
                    key_press.state
                );
            }
            x11rb::protocol::Event::PropertyNotify(prop_notify) => {
                // Property changes might indicate IME state changes
                log_debug!(
                    "PropertyNotify: atom={}, state={:?}",
                    prop_notify.atom,
                    prop_notify.state
                );
            }
            x11rb::protocol::Event::MappingNotify(_) => {
                // Keyboard mapping changed - refresh our cache
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

    /// Check if the input method is active
    pub fn is_active(&self) -> bool {
        self.state.active
    }

    /// Commit text to the focused window using XTest extension
    pub fn commit_string(&self, text: &str) -> Result<()> {
        let mapping = self
            .keyboard_mapping
            .as_ref()
            .ok_or_else(|| Error::ConnectionFailed("No keyboard mapping".to_string()))?;

        for c in text.chars() {
            // Convert character to keysym
            let keysym = char_to_keysym(c);
            if keysym == 0 {
                log_warn!("No keysym for character: {:?}", c);
                continue;
            }

            // Find keycode for this keysym
            if let Some((keycode, needs_shift)) = Self::find_keycode_for_keysym_in_mapping(
                keysym,
                &mapping.keysyms,
                mapping.keysyms_per_keycode,
                mapping.min_keycode,
            ) {
                self.send_key_with_mapping(keycode, needs_shift, mapping)?;
            } else {
                // Fallback: use Unicode input (Ctrl+Shift+U)
                self.send_unicode_input_with_mapping(c, mapping)?;
            }
        }

        // Flush to ensure all events are sent
        self.connection
            .flush()
            .map_err(|e| Error::CommitFailed(e.to_string()))?;

        Ok(())
    }

    /// Find keycode for a keysym in the mapping
    fn find_keycode_for_keysym_in_mapping(
        keysym: Keysym,
        keysyms: &[Keysym],
        keysyms_per_keycode: usize,
        min_keycode: Keycode,
    ) -> Option<(Keycode, bool)> {
        for (i, chunk) in keysyms.chunks(keysyms_per_keycode).enumerate() {
            let keycode = min_keycode + i as u8;
            // Check unshifted (index 0) and shifted (index 1) keysyms
            if !chunk.is_empty() && chunk[0] == keysym {
                return Some((keycode, false));
            }
            if chunk.len() > 1 && chunk[1] == keysym {
                return Some((keycode, true));
            }
        }
        None
    }

    /// Send a key press and release with mapping
    fn send_key_with_mapping(
        &self,
        keycode: Keycode,
        needs_shift: bool,
        mapping: &CachedKeyboardMapping,
    ) -> Result<()> {
        // Find the shift keycode dynamically from keysym
        let shift_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_SHIFT_L,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(50); // Fallback to common value if not found

        // Press shift if needed
        if needs_shift {
            xtest::fake_input(
                &self.connection,
                KEY_PRESS_EVENT,
                shift_keycode,
                x11rb::CURRENT_TIME,
                self.root_window,
                0,
                0,
                0,
            )
            .map_err(|e| Error::CommitFailed(e.to_string()))?;
        }

        // Key press
        xtest::fake_input(
            &self.connection,
            KEY_PRESS_EVENT,
            keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;

        // Key release
        xtest::fake_input(
            &self.connection,
            KEY_RELEASE_EVENT,
            keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;

        // Release shift if needed
        if needs_shift {
            xtest::fake_input(
                &self.connection,
                KEY_RELEASE_EVENT,
                shift_keycode,
                x11rb::CURRENT_TIME,
                self.root_window,
                0,
                0,
                0,
            )
            .map_err(|e| Error::CommitFailed(e.to_string()))?;
        }

        Ok(())
    }

    /// Send Unicode input using Ctrl+Shift+U method (for GTK/Qt apps)
    fn send_unicode_input_with_mapping(
        &self,
        c: char,
        mapping: &CachedKeyboardMapping,
    ) -> Result<()> {
        let code = c as u32;
        let hex = format!("{:x}", code);

        // Find keycodes dynamically from keysyms
        let ctrl_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_CONTROL_L,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(37);

        let shift_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_SHIFT_L,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(50);

        let u_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_U,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(30);

        let space_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_SPACE,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(65);

        // Press Ctrl+Shift+U
        xtest::fake_input(
            &self.connection,
            KEY_PRESS_EVENT,
            ctrl_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
        xtest::fake_input(
            &self.connection,
            KEY_PRESS_EVENT,
            shift_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
        xtest::fake_input(
            &self.connection,
            KEY_PRESS_EVENT,
            u_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
        xtest::fake_input(
            &self.connection,
            KEY_RELEASE_EVENT,
            u_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
        xtest::fake_input(
            &self.connection,
            KEY_RELEASE_EVENT,
            shift_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
        xtest::fake_input(
            &self.connection,
            KEY_RELEASE_EVENT,
            ctrl_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;

        // Type hex code
        for h in hex.chars() {
            let keysym = char_to_keysym(h);
            if let Some((keycode, needs_shift)) = Self::find_keycode_for_keysym_in_mapping(
                keysym,
                &mapping.keysyms,
                mapping.keysyms_per_keycode,
                mapping.min_keycode,
            ) {
                self.send_key_with_mapping(keycode, needs_shift, mapping)?;
            }
        }

        // Press space to confirm
        xtest::fake_input(
            &self.connection,
            KEY_PRESS_EVENT,
            space_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
        xtest::fake_input(
            &self.connection,
            KEY_RELEASE_EVENT,
            space_keycode,
            x11rb::CURRENT_TIME,
            self.root_window,
            0,
            0,
            0,
        )
        .map_err(|e| Error::CommitFailed(e.to_string()))?;

        Ok(())
    }

    /// Set preedit string (not directly supported in XTest mode)
    pub fn set_preedit_string(
        &self,
        _text: &str,
        _cursor_begin: i32,
        _cursor_end: i32,
    ) -> Result<()> {
        // XTest doesn't support preedit - it's a limitation
        log_debug!("Preedit not supported in X11 XTest mode");
        Ok(())
    }

    /// Delete surrounding text by sending backspace/delete keys
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        let mapping = self
            .keyboard_mapping
            .as_ref()
            .ok_or_else(|| Error::ConnectionFailed("No keyboard mapping".to_string()))?;

        // Find keycodes dynamically from keysyms
        let backspace_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_BACKSPACE,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(22);

        let delete_keycode = Self::find_keycode_for_keysym_in_mapping(
            XK_DELETE,
            &mapping.keysyms,
            mapping.keysyms_per_keycode,
            mapping.min_keycode,
        )
        .map(|(kc, _)| kc)
        .unwrap_or(119);

        // Delete before cursor
        for _ in 0..before {
            self.send_key_with_mapping(backspace_keycode, false, mapping)?;
        }

        // Delete after cursor
        for _ in 0..after {
            self.send_key_with_mapping(delete_keycode, false, mapping)?;
        }

        self.connection
            .flush()
            .map_err(|e| Error::CommitFailed(e.to_string()))?;

        Ok(())
    }

    /// Commit changes (no-op for X11 as commits are immediate)
    pub fn commit(&self, _serial: u32) -> Result<()> {
        // X11 commits are immediate
        Ok(())
    }

    /// Grab keyboard (not supported in XTest mode)
    pub fn grab_keyboard(&mut self) -> Result<()> {
        Err(Error::ProtocolNotSupported(
            "Keyboard grab not supported in X11 XTest mode".to_string(),
        ))
    }

    /// Get the current state
    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}

/// Convert a character to its X11 keysym
fn char_to_keysym(c: char) -> Keysym {
    // Basic ASCII characters map directly to their values
    // X11 keysyms for Latin-1 characters are their Unicode values
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
