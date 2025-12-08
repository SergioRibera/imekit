//! Windows Input Method implementation using Text Services Framework (TSF)
//!
//! This module implements IME functionality using the Windows TSF API
//! and SendInput for text injection.
//!
//! TSF is the modern input method framework for Windows. This implementation
//! monitors IME events using the Windows message queue and uses SendInput
//! for text injection.

use std::collections::VecDeque;
use std::ptr::null_mut;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    WM_IME_CHAR, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_NOTIFY, WM_IME_SETCONTEXT,
    WM_IME_STARTCOMPOSITION, WM_INPUTLANGCHANGE,
};

use crate::{Error, InputMethodEvent, InputMethodState, Result};

/// Windows input method implementation using TSF and SendInput
pub struct InputMethod {
    active: bool,
    serial: u32,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
    composing: bool,
}

impl InputMethod {
    /// Create a new input method instance
    pub fn new() -> Result<Self> {
        Ok(Self {
            active: true, // Always active on Windows since we use SendInput
            serial: 0,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            composing: false,
        })
    }

    /// Get the next event by polling Windows message queue for IME events
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        // First return any queued events
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }

        // Poll for IME-related messages
        unsafe {
            let mut msg: MSG = std::mem::zeroed();

            // Non-blocking peek for messages
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                // Check for IME messages
                if let Some(event) = self.process_message(&msg) {
                    // Translate and dispatch the message
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                    return Some(event);
                }

                // Translate and dispatch non-IME messages
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }

        None
    }

    /// Process a Windows message and convert to InputMethodEvent if applicable
    fn process_message(&mut self, msg: &MSG) -> Option<InputMethodEvent> {
        match msg.message {
            WM_IME_STARTCOMPOSITION => {
                if !self.composing {
                    self.composing = true;
                    self.serial += 1;
                    self.state.active = true;
                    return Some(InputMethodEvent::Activate {
                        serial: self.serial,
                    });
                }
            }
            WM_IME_ENDCOMPOSITION => {
                if self.composing {
                    self.composing = false;
                    return Some(InputMethodEvent::Deactivate);
                }
            }
            WM_IME_COMPOSITION => {
                // Get the LPARAM flags
                let flags = msg.lParam.0 as u32;
                // GCS_COMPSTR = 0x0008, GCS_RESULTSTR = 0x0800
                const GCS_COMPSTR: u32 = 0x0008;
                const GCS_RESULTSTR: u32 = 0x0800;

                if (flags & GCS_RESULTSTR) != 0 {
                    // Result string available - this is the committed text
                    return Some(InputMethodEvent::Done);
                }
                if (flags & GCS_COMPSTR) != 0 {
                    // Composition string changed
                    // Would need to call ImmGetCompositionString to get the actual text
                    log_debug!("IME composition string changed");
                }
            }
            WM_IME_NOTIFY => {
                // IME notification - various subcommands
                let command = msg.wParam.0 as u32;
                log_debug!("WM_IME_NOTIFY command: {}", command);
            }
            WM_IME_SETCONTEXT => {
                // IME context is being set
                let active = msg.wParam.0 != 0;
                if active && !self.state.active {
                    self.serial += 1;
                    self.state.active = true;
                    return Some(InputMethodEvent::Activate {
                        serial: self.serial,
                    });
                } else if !active && self.state.active {
                    self.state.active = false;
                    return Some(InputMethodEvent::Deactivate);
                }
            }
            WM_INPUTLANGCHANGE => {
                // Input language changed
                log_debug!("Input language changed");
            }
            WM_IME_CHAR => {
                // Character from IME
                let char_code = msg.wParam.0 as u16;
                if let Some(c) = char::from_u32(char_code as u32) {
                    log_debug!("WM_IME_CHAR: {}", c);
                }
            }
            _ => {}
        }

        None
    }

    /// Check if active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the foreground window
    fn get_foreground_window() -> HWND {
        unsafe { GetForegroundWindow() }
    }

    /// Commit text using SendInput with Unicode characters
    pub fn commit_string(&self, text: &str) -> Result<()> {
        // Check if there's a foreground window
        let hwnd = Self::get_foreground_window();
        if hwnd.0.is_null() {
            return Err(Error::NotActive);
        }

        // Send each character as a Unicode input
        for c in text.chars() {
            // Get the full u32 codepoint first to determine BMP vs non-BMP
            let codepoint = c as u32;

            // For characters that fit in BMP (U+0000 to U+FFFF), send directly
            // For characters outside BMP, send as surrogate pairs
            if codepoint <= 0xFFFF {
                self.send_unicode_char(codepoint as u16)?;
            } else {
                // Encode as UTF-16 surrogate pair
                let high = ((codepoint - 0x10000) >> 10) as u16 + 0xD800;
                let low = ((codepoint - 0x10000) & 0x3FF) as u16 + 0xDC00;
                self.send_unicode_char(high)?;
                self.send_unicode_char(low)?;
            }
        }

        Ok(())
    }

    /// Send a single Unicode character
    fn send_unicode_char(&self, code: u16) -> Result<()> {
        let inputs = [
            // Key down
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: code,
                        dwFlags: KEYEVENTF_UNICODE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            // Key up
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: code,
                        dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];

        let result = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

        if result != inputs.len() as u32 {
            return Err(Error::CommitFailed("SendInput failed".to_string()));
        }

        Ok(())
    }

    /// Send a virtual key press
    fn send_key(&self, vk: u16) -> Result<()> {
        let inputs = [
            // Key down
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: KEYBD_EVENT_FLAGS(0),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            // Key up
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];

        let result = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

        if result != inputs.len() as u32 {
            return Err(Error::CommitFailed("SendInput failed".to_string()));
        }

        Ok(())
    }

    /// Set preedit string (not directly supported with SendInput)
    pub fn set_preedit_string(
        &self,
        _text: &str,
        _cursor_begin: i32,
        _cursor_end: i32,
    ) -> Result<()> {
        // SendInput doesn't support preedit display
        log_debug!("Preedit not supported in Windows SendInput mode");
        Ok(())
    }

    /// Delete surrounding text by sending backspace/delete keys
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        const VK_BACK: u16 = 0x08;
        const VK_DELETE: u16 = 0x2E;

        // Delete before cursor
        for _ in 0..before {
            self.send_key(VK_BACK)?;
        }

        // Delete after cursor
        for _ in 0..after {
            self.send_key(VK_DELETE)?;
        }

        Ok(())
    }

    /// Commit changes (no-op for Windows as commits are immediate)
    pub fn commit(&self, _serial: u32) -> Result<()> {
        Ok(())
    }

    /// Get the current state
    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}
