//! Windows Input Method implementation
//!
//! Text injection via `SendInput`; preedit via IMM32 `ImmSetCompositionStringW`.
//! A proper TSF `ITfTextInputProcessor` requires a registered COM text service (DLL).
//! That architecture is outside this library's scope; `SendInput` is the correct
//! in-process approach for an embedded IME library.

use std::collections::VecDeque;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::Ime::{
    ImmGetContext, ImmReleaseContext, ImmSetCompositionStringW, HIMC, SCS_SETSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION,
};

use crate::{Error, InputMethodEvent, InputMethodState, Result};

pub struct InputMethod {
    active: bool,
    serial: u32,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
    composing: bool,
}

impl InputMethod {
    pub fn new() -> Result<Self> {
        Ok(Self {
            active: true,
            serial: 0,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            composing: false,
        })
    }

    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if let Some(event) = self.process_message(&msg) {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                    return Some(event);
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
        None
    }

    fn process_message(&mut self, msg: &MSG) -> Option<InputMethodEvent> {
        match msg.message {
            WM_IME_STARTCOMPOSITION => {
                if !self.composing {
                    self.composing = true;
                    self.serial += 1;
                    self.state.active = true;
                    return Some(InputMethodEvent::Activate { serial: self.serial });
                }
            }
            WM_IME_ENDCOMPOSITION => {
                if self.composing {
                    self.composing = false;
                    return Some(InputMethodEvent::Deactivate);
                }
            }
            WM_IME_SETCONTEXT => {
                let active = msg.wParam.0 != 0;
                if active && !self.state.active {
                    self.serial += 1;
                    self.state.active = true;
                    return Some(InputMethodEvent::Activate { serial: self.serial });
                } else if !active && self.state.active {
                    self.state.active = false;
                    return Some(InputMethodEvent::Deactivate);
                }
            }
            _ => {}
        }
        None
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn foreground_window() -> HWND {
        unsafe { GetForegroundWindow() }
    }

    /// Commit text via `SendInput` Unicode events.
    pub fn commit_string(&self, text: &str) -> Result<()> {
        let hwnd = Self::foreground_window();
        if hwnd.0.is_null() {
            return Err(Error::NotActive);
        }
        for c in text.chars() {
            let cp = c as u32;
            if cp <= 0xFFFF {
                self.send_unicode_char(cp as u16)?;
            } else {
                let high = ((cp - 0x10000) >> 10) as u16 + 0xD800;
                let low = ((cp - 0x10000) & 0x3FF) as u16 + 0xDC00;
                self.send_unicode_char(high)?;
                self.send_unicode_char(low)?;
            }
        }
        Ok(())
    }

    fn send_unicode_char(&self, code: u16) -> Result<()> {
        let inputs = [
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
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent != inputs.len() as u32 {
            return Err(Error::CommitFailed("SendInput failed".to_string()));
        }
        Ok(())
    }

    fn send_vk(&self, vk: u16) -> Result<()> {
        let inputs = [
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
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent != inputs.len() as u32 {
            return Err(Error::CommitFailed("SendInput failed".to_string()));
        }
        Ok(())
    }

    /// Set preedit via IMM32 `ImmSetCompositionStringW`.
    ///
    /// Works for IMM-compatible apps. TSF-only apps ignore IMM composition.
    pub fn set_preedit_string(&self, text: &str, _cursor_begin: i32, _cursor_end: i32) -> Result<()> {
        let hwnd = Self::foreground_window();
        if hwnd.0.is_null() {
            return Err(Error::NotActive);
        }
        unsafe {
            let himc = ImmGetContext(hwnd);
            if himc == HIMC(std::ptr::null_mut()) {
                log_debug!("ImmGetContext returned null — IMM not available for this window");
                return Ok(());
            }
            let wide: Vec<u16> = text.encode_utf16().collect();
            let _ = ImmSetCompositionStringW(
                himc,
                SCS_SETSTR,
                Some(wide.as_ptr() as _),
                (wide.len() * 2) as u32,
                None,
                0,
            );
            ImmReleaseContext(hwnd, himc);
        }
        Ok(())
    }

    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        const VK_BACK: u16 = 0x08;
        const VK_DELETE: u16 = 0x2E;
        for _ in 0..before {
            self.send_vk(VK_BACK)?;
        }
        for _ in 0..after {
            self.send_vk(VK_DELETE)?;
        }
        Ok(())
    }

    pub fn commit(&self, _serial: u32) -> Result<()> {
        Ok(())
    }

    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}
