//! macOS Input Method — IMK engine mode + CGEvent fallback
//!
//! ## Engine mode (`new_as_engine`)
//! Call this from your `IMKInputController` subclass. The system passes an
//! `id<IMKTextInput>` client pointer; engine mode uses `insertText:replacementRange:`
//! and `setMarkedText:selectedRange:replacementRange:` for proper preedit support
//! without Accessibility permissions.
//!
//! ## Standalone mode (`new`)
//! Falls back to `CGEvent`-based key injection, which requires Accessibility
//! permissions. Preedit is not supported in standalone mode.
//!
//! Full IMK integration requires your binary to be packaged as an Input Method
//! bundle (`/Library/Input Methods/`) with an appropriate `Info.plist`.

use std::collections::VecDeque;
use std::ffi::c_void;

use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSRange, NSString};

use crate::{Error, InputMethodEvent, InputMethodState, Result};

// CoreGraphics FFI for standalone mode fallback
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGEventKeyboardSetUnicodeString(event: *mut c_void, length: u64, chars: *const u16);
    fn CFRelease(cf: *mut c_void);
}

const K_VK_DELETE: u16 = 0x33;
const K_VK_FORWARD_DELETE: u16 = 0x75;
const K_CG_HID_EVENT_TAP: u32 = 0;

enum Mode {
    /// IMK engine mode — raw pointer to `id<IMKTextInput>` client.
    ///
    /// The pointer is valid for the lifetime of the `IMKInputController`
    /// session; callers must ensure the controller outlives this struct.
    Engine { client: *mut AnyObject },
    /// Standalone CGEvent fallback.
    CgEvent,
}

// SAFETY: AnyObject pointers are safe to move across threads when guarded
// by ObjC retain semantics; callers are responsible for keeping the client alive.
unsafe impl Send for Mode {}
unsafe impl Sync for Mode {}

pub struct InputMethod {
    mode: Mode,
    active: bool,
    serial: u32,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
}

impl InputMethod {
    /// Standalone mode — uses `CGEvent` injection (requires Accessibility permissions).
    pub fn new() -> Result<Self> {
        Ok(Self {
            mode: Mode::CgEvent,
            active: true,
            serial: 0,
            state: InputMethodState::new(),
            events: VecDeque::new(),
        })
    }

    /// IMK engine mode — call from `IMKInputController initWithServer:delegate:client:`.
    ///
    /// `client` must be an `id<IMKTextInput>` retained by the caller.
    pub fn new_as_engine(client: *mut AnyObject) -> Self {
        Self {
            mode: Mode::Engine { client },
            active: true,
            serial: 0,
            state: InputMethodState::new(),
            events: VecDeque::new(),
        }
    }

    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        self.events.pop_front()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Commit text.
    ///
    /// Engine mode: `insertText:replacementRange:` on the IMK client.
    /// Standalone mode: `CGEvent` Unicode injection.
    pub fn commit_string(&self, text: &str) -> Result<()> {
        match self.mode {
            Mode::Engine { client } => {
                let ns_str = NSString::from_str(text);
                // NSNotFound (usize::MAX) signals no replacement range — insert at cursor.
                let range = NSRange { location: usize::MAX, length: 0 };
                unsafe {
                    let _: () = msg_send![client, insertText: &*ns_str, replacementRange: range];
                }
                Ok(())
            }
            Mode::CgEvent => self.cg_commit(text),
        }
    }

    /// Set preedit / marked text.
    ///
    /// Engine mode: `setMarkedText:selectedRange:replacementRange:` on the IMK client.
    /// Standalone mode: no-op (CGEvent has no preedit channel).
    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, cursor_end: i32) -> Result<()> {
        match self.mode {
            Mode::Engine { client } => {
                let ns_str = NSString::from_str(text);
                let sel_range = NSRange {
                    location: cursor_begin.max(0) as usize,
                    length: (cursor_end - cursor_begin).max(0) as usize,
                };
                let repl_range = NSRange { location: usize::MAX, length: 0 };
                unsafe {
                    let _: () = msg_send![
                        client,
                        setMarkedText: &*ns_str,
                        selectedRange: sel_range,
                        replacementRange: repl_range
                    ];
                }
                Ok(())
            }
            Mode::CgEvent => {
                log_debug!("preedit not supported in CGEvent mode — package as IMK bundle");
                Ok(())
            }
        }
    }

    /// Delete surrounding text via `CGEvent` backspace/forward-delete keys.
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        unsafe {
            for _ in 0..before {
                cg_key(K_VK_DELETE);
            }
            for _ in 0..after {
                cg_key(K_VK_FORWARD_DELETE);
            }
        }
        Ok(())
    }

    pub fn commit(&self, _serial: u32) -> Result<()> {
        Ok(())
    }

    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }

    fn cg_commit(&self, text: &str) -> Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), 0, true);
            if ev.is_null() {
                return Err(Error::CommitFailed("CGEventCreateKeyboardEvent failed".into()));
            }
            CGEventKeyboardSetUnicodeString(ev, utf16.len() as u64, utf16.as_ptr());
            CGEventPost(K_CG_HID_EVENT_TAP, ev);
            CFRelease(ev);

            let ev_up = CGEventCreateKeyboardEvent(std::ptr::null(), 0, false);
            if !ev_up.is_null() {
                CGEventKeyboardSetUnicodeString(ev_up, utf16.len() as u64, utf16.as_ptr());
                CGEventPost(K_CG_HID_EVENT_TAP, ev_up);
                CFRelease(ev_up);
            }
        }
        Ok(())
    }
}

unsafe fn cg_key(vk: u16) {
    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), vk, true);
    if !ev.is_null() {
        CGEventPost(K_CG_HID_EVENT_TAP, ev);
        CFRelease(ev);
    }
    let ev_up = CGEventCreateKeyboardEvent(std::ptr::null(), vk, false);
    if !ev_up.is_null() {
        CGEventPost(K_CG_HID_EVENT_TAP, ev_up);
        CFRelease(ev_up);
    }
}
