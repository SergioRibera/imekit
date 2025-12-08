//! macOS Input Method implementation
//!
//! This module implements IME functionality using CoreGraphics CGEvent
//! for text injection on macOS.
//!
//! Note: Using CGEvent requires accessibility permissions.
//! The user will be prompted to grant access on first use.

use std::collections::VecDeque;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEvent, NSEventType};
use objc2_foundation::MainThreadMarker;

use crate::{Error, InputMethodEvent, InputMethodState, Result};

// CoreGraphics FFI for CGEvent
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

// Carbon virtual key codes
const K_VK_DELETE: u16 = 0x33; // Backspace
const K_VK_FORWARD_DELETE: u16 = 0x75;

// CGEventTapLocation
const K_CG_HID_EVENT_TAP: u32 = 0;

/// macOS input method implementation using CGEvent
pub struct InputMethod {
    active: bool,
    serial: u32,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
    /// Main thread marker for AppKit
    #[allow(dead_code)]
    mtm: Option<MainThreadMarker>,
}

impl InputMethod {
    /// Create a new input method instance
    ///
    /// Note: Using CGEvent for text injection requires accessibility permissions.
    pub fn new() -> Result<Self> {
        // Try to get the main thread marker for event handling
        let mtm = MainThreadMarker::new();

        Ok(Self {
            active: true, // Always active on macOS since we use CGEvent
            serial: 0,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            mtm,
        })
    }

    /// Get the next event by polling NSApplication events
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        // First return any queued events
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }

        // Try to get events from NSApplication if we have main thread access
        if let Some(mtm) = self.mtm {
            // Safety: We verified we're on the main thread
            let app = NSApplication::sharedApplication(mtm);

            // Poll for events (non-blocking)
            // Use a very short timeout to avoid blocking
            loop {
                let event: Option<Retained<NSEvent>> = unsafe {
                    app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        objc2_app_kit::NSEventMask::Any,
                        None, // No wait
                        objc2_foundation::NSDefaultRunLoopMode,
                        true,
                    )
                };

                match event {
                    Some(ns_event) => {
                        // Convert NSEvent to InputMethodEvent if applicable
                        if let Some(ime_event) = self.convert_ns_event(&ns_event) {
                            return Some(ime_event);
                        }
                        // Continue polling if this event wasn't IME-related
                    }
                    None => break, // No more events
                }
            }
        }

        None
    }

    /// Convert NSEvent to InputMethodEvent
    fn convert_ns_event(&mut self, ns_event: &NSEvent) -> Option<InputMethodEvent> {
        let event_type = ns_event.r#type();

        match event_type {
            NSEventType::KeyDown => {
                // Check for text input events
                if let Some(characters) = ns_event.characters() {
                    let text = characters.to_string();
                    if !text.is_empty() {
                        // This is a text input event
                        if !self.state.active {
                            self.state.active = true;
                            self.serial += 1;
                            return Some(InputMethodEvent::Activate {
                                serial: self.serial,
                            });
                        }
                    }
                }
                None
            }
            NSEventType::FlagsChanged => {
                // Modifier key changes might indicate IME state changes
                None
            }
            _ => None,
        }
    }

    /// Check if active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Commit text using CGEvent keyboard simulation
    pub fn commit_string(&self, text: &str) -> Result<()> {
        // Convert text to UTF-16 for CGEvent
        let utf16: Vec<u16> = text.encode_utf16().collect();

        // Send text via CGEvent
        unsafe {
            // Create a keyboard event with a dummy keycode (we'll override with Unicode)
            let event = CGEventCreateKeyboardEvent(std::ptr::null(), 0, true);
            if event.is_null() {
                return Err(Error::CommitFailed(
                    "Failed to create keyboard event".to_string(),
                ));
            }

            // Set the Unicode string for this event
            CGEventKeyboardSetUnicodeString(event, utf16.len() as u64, utf16.as_ptr());

            // Post the event
            CGEventPost(K_CG_HID_EVENT_TAP, event);
            CFRelease(event);

            // Key up event
            let event_up = CGEventCreateKeyboardEvent(std::ptr::null(), 0, false);
            if !event_up.is_null() {
                CGEventKeyboardSetUnicodeString(event_up, utf16.len() as u64, utf16.as_ptr());
                CGEventPost(K_CG_HID_EVENT_TAP, event_up);
                CFRelease(event_up);
            }
        }

        Ok(())
    }

    /// Set preedit string (marked text)
    pub fn set_preedit_string(
        &self,
        _text: &str,
        _cursor_begin: i32,
        _cursor_end: i32,
    ) -> Result<()> {
        // Preedit requires integration with the active text view via NSTextInputClient
        // This is more complex and requires knowing the target application
        log_debug!("Preedit not fully supported in macOS CGEvent mode - would need NSTextInputClient integration");
        Ok(())
    }

    /// Delete surrounding text using native key events
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        unsafe {
            // Delete before cursor with backspace
            for _ in 0..before {
                // Key down
                let event = CGEventCreateKeyboardEvent(std::ptr::null(), K_VK_DELETE, true);
                if !event.is_null() {
                    CGEventPost(K_CG_HID_EVENT_TAP, event);
                    CFRelease(event);
                }
                // Key up
                let event_up = CGEventCreateKeyboardEvent(std::ptr::null(), K_VK_DELETE, false);
                if !event_up.is_null() {
                    CGEventPost(K_CG_HID_EVENT_TAP, event_up);
                    CFRelease(event_up);
                }
            }

            // Delete after cursor with forward delete
            for _ in 0..after {
                // Key down
                let event = CGEventCreateKeyboardEvent(std::ptr::null(), K_VK_FORWARD_DELETE, true);
                if !event.is_null() {
                    CGEventPost(K_CG_HID_EVENT_TAP, event);
                    CFRelease(event);
                }
                // Key up
                let event_up =
                    CGEventCreateKeyboardEvent(std::ptr::null(), K_VK_FORWARD_DELETE, false);
                if !event_up.is_null() {
                    CGEventPost(K_CG_HID_EVENT_TAP, event_up);
                    CFRelease(event_up);
                }
            }
        }

        Ok(())
    }

    /// Commit changes (no-op for macOS as commits are immediate)
    pub fn commit(&self, _serial: u32) -> Result<()> {
        Ok(())
    }

    /// Get the current state
    pub fn state(&self) -> InputMethodState {
        self.state.clone()
    }
}
