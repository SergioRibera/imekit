//! Wayland Input Method v2 implementation
//!
//! This module implements the `zwp_input_method_v2` protocol for Wayland.
//! The protocol allows applications to act as input methods.

use std::sync::{Arc, Mutex};

use wayland_client::{
    protocol::{wl_registry, wl_seat},
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2, zwp_input_method_manager_v2, zwp_input_method_v2,
    zwp_input_popup_surface_v2,
};

use crate::{
    ChangeCause, ContentHint, ContentPurpose, Error, InputMethodEvent, InputMethodState, Result,
};

/// Wayland input method implementation using zwp_input_method_v2
pub struct InputMethod {
    connection: Connection,
    event_queue: EventQueue<InputMethodData>,
    data: Arc<Mutex<InputMethodData>>,
}

/// Internal data for the input method
#[allow(dead_code)]
pub struct InputMethodData {
    /// Input method manager
    manager: Option<zwp_input_method_manager_v2::ZwpInputMethodManagerV2>,
    /// Seat
    seat: Option<wl_seat::WlSeat>,
    /// The input method instance
    input_method: Option<zwp_input_method_v2::ZwpInputMethodV2>,
    /// Keyboard grab
    keyboard_grab: Option<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2>,
    /// Popup surface
    popup_surface: Option<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2>,
    /// Current state
    state: InputMethodState,
    /// Pending events
    events: Vec<InputMethodEvent>,
}

impl Default for InputMethodData {
    fn default() -> Self {
        Self {
            manager: None,
            seat: None,
            input_method: None,
            keyboard_grab: None,
            popup_surface: None,
            state: InputMethodState::new(),
            events: Vec::new(),
        }
    }
}

impl InputMethod {
    /// Create a new input method instance
    ///
    /// This connects to the Wayland display and binds to the
    /// `zwp_input_method_manager_v2` global.
    pub fn new() -> Result<Self> {
        let connection = Connection::connect_to_env()?;
        let display = connection.display();

        let data = Arc::new(Mutex::new(InputMethodData::default()));
        let mut event_queue = connection.new_event_queue();
        let qh = event_queue.handle();

        // Get the registry and enumerate globals
        display.get_registry(&qh, ());

        // Do a roundtrip to get the globals
        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        // Check if we got the manager
        {
            let data = data.lock().unwrap();
            if data.manager.is_none() {
                return Err(Error::ProtocolNotSupported(
                    "zwp_input_method_manager_v2 not available".to_string(),
                ));
            }
            if data.seat.is_none() {
                return Err(Error::ConnectionFailed("No seat found".to_string()));
            }
        }

        // Create the input method
        {
            let mut data_guard = data.lock().unwrap();
            if let (Some(manager), Some(seat)) = (&data_guard.manager, &data_guard.seat) {
                let im = manager.get_input_method(seat, &qh, ());
                data_guard.input_method = Some(im);
            }
        }

        // Another roundtrip to setup the input method
        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        Ok(Self {
            connection,
            event_queue,
            data,
        })
    }

    /// Get the next event from the input method
    ///
    /// This dispatches pending Wayland events and returns the next
    /// input method event if available.
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        // Dispatch pending events
        let _ = self
            .event_queue
            .dispatch_pending(&mut *self.data.lock().unwrap());

        // Try to get more events
        if let Some(guard) = self.connection.prepare_read() {
            if let Err(e) = guard.read() {
                log_warn!("Wayland connection read error: {}", e);
            }
        }
        let _ = self
            .event_queue
            .dispatch_pending(&mut *self.data.lock().unwrap());

        // Return the next event
        self.data.lock().unwrap().events.pop()
    }

    /// Dispatch events (blocking)
    pub fn dispatch(&mut self) -> Result<()> {
        self.event_queue
            .blocking_dispatch(&mut *self.data.lock().unwrap())?;
        Ok(())
    }

    /// Check if the input method is active
    pub fn is_active(&self) -> bool {
        self.data.lock().unwrap().state.active
    }

    /// Commit text to the client
    pub fn commit_string(&self, text: &str) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.commit_string(text.to_string());
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Set preedit text (composing text shown to user)
    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, cursor_end: i32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.set_preedit_string(text.to_string(), cursor_begin, cursor_end);
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Delete surrounding text
    pub fn delete_surrounding_text(&self, before_length: u32, after_length: u32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.delete_surrounding_text(before_length, after_length);
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Commit all pending changes
    pub fn commit(&self, serial: u32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.commit(serial);
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Grab the keyboard
    pub fn grab_keyboard(&mut self) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            let qh = self.event_queue.handle();
            let grab = im.grab_keyboard(&qh, ());
            data.keyboard_grab = Some(grab);
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Get the current state
    pub fn state(&self) -> InputMethodState {
        let data = self.data.lock().unwrap();
        InputMethodState {
            active: data.state.active,
            serial: data.state.serial,
            surrounding_text: data.state.surrounding_text.clone(),
            cursor: data.state.cursor,
            anchor: data.state.anchor,
            content_hint: data.state.content_hint,
            content_purpose: data.state.content_purpose,
            change_cause: data.state.change_cause,
            preedit_text: data.state.preedit_text.clone(),
            preedit_cursor: data.state.preedit_cursor,
            commit_text: data.state.commit_text.clone(),
            delete_before: data.state.delete_before,
            delete_after: data.state.delete_after,
        }
    }
}

// Helper to parse content hint from raw u32 value
fn parse_content_hint(hint_raw: u32) -> ContentHint {
    ContentHint {
        completion: hint_raw & 0x1 != 0,
        spellcheck: hint_raw & 0x2 != 0,
        auto_capitalization: hint_raw & 0x4 != 0,
        lowercase: hint_raw & 0x8 != 0,
        uppercase: hint_raw & 0x10 != 0,
        titlecase: hint_raw & 0x20 != 0,
        hidden_text: hint_raw & 0x40 != 0,
        sensitive_data: hint_raw & 0x80 != 0,
        latin: hint_raw & 0x100 != 0,
        multiline: hint_raw & 0x200 != 0,
    }
}

// Helper to parse content purpose from WEnum
fn parse_content_purpose(
    purpose: WEnum<
        wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ContentPurpose,
    >,
) -> ContentPurpose {
    use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ContentPurpose as WlPurpose;
    match purpose {
        WEnum::Value(WlPurpose::Alpha) => ContentPurpose::Alpha,
        WEnum::Value(WlPurpose::Digits) => ContentPurpose::Digits,
        WEnum::Value(WlPurpose::Number) => ContentPurpose::Number,
        WEnum::Value(WlPurpose::Phone) => ContentPurpose::Phone,
        WEnum::Value(WlPurpose::Url) => ContentPurpose::Url,
        WEnum::Value(WlPurpose::Email) => ContentPurpose::Email,
        WEnum::Value(WlPurpose::Name) => ContentPurpose::Name,
        WEnum::Value(WlPurpose::Password) => ContentPurpose::Password,
        WEnum::Value(WlPurpose::Pin) => ContentPurpose::Pin,
        WEnum::Value(WlPurpose::Date) => ContentPurpose::Date,
        WEnum::Value(WlPurpose::Time) => ContentPurpose::Time,
        WEnum::Value(WlPurpose::Datetime) => ContentPurpose::Datetime,
        WEnum::Value(WlPurpose::Terminal) => ContentPurpose::Terminal,
        _ => ContentPurpose::Normal,
    }
}

// Registry dispatch
impl Dispatch<wl_registry::WlRegistry, ()> for InputMethodData {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "zwp_input_method_manager_v2" => {
                    let manager = registry
                        .bind::<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        );
                    state.manager = Some(manager);
                }
                "wl_seat" => {
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(8), qh, ());
                    state.seat = Some(seat);
                }
                _ => {}
            }
        }
    }
}

// Seat dispatch
impl Dispatch<wl_seat::WlSeat, ()> for InputMethodData {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // We don't need to handle seat events for now
    }
}

// Input method manager dispatch
impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for InputMethodData {
    fn event(
        _state: &mut Self,
        _manager: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _event: zwp_input_method_manager_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No events for the manager
    }
}

// Input method dispatch
impl Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()> for InputMethodData {
    fn event(
        state: &mut Self,
        _im: &zwp_input_method_v2::ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                state.state.active = true;
                state.state.serial += 1;
                state.events.push(InputMethodEvent::Activate {
                    serial: state.state.serial,
                });
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.state.active = false;
                state.state.reset();
                state.events.push(InputMethodEvent::Deactivate);
            }
            zwp_input_method_v2::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                state.state.surrounding_text = Some(text.clone());
                state.state.cursor = cursor;
                state.state.anchor = anchor;
                state.events.push(InputMethodEvent::SurroundingText {
                    text,
                    cursor,
                    anchor,
                });
            }
            zwp_input_method_v2::Event::TextChangeCause { cause } => {
                use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ChangeCause as WlChangeCause;
                let cause = match cause {
                    WEnum::Value(WlChangeCause::InputMethod) => ChangeCause::InputMethod,
                    _ => ChangeCause::Other,
                };
                state.state.change_cause = cause;
                state.events.push(InputMethodEvent::TextChangeCause(cause));
            }
            zwp_input_method_v2::Event::ContentType { hint, purpose } => {
                // hint is WEnum<ContentHint> - we need to extract raw value
                let hint_raw = match hint {
                    WEnum::Value(h) => h.bits(),
                    WEnum::Unknown(v) => v,
                };
                let content_hint = parse_content_hint(hint_raw);
                let content_purpose = parse_content_purpose(purpose);

                state.state.content_hint = content_hint;
                state.state.content_purpose = content_purpose;
                state.events.push(InputMethodEvent::ContentType {
                    hint: content_hint,
                    purpose: content_purpose,
                });
            }
            zwp_input_method_v2::Event::Done => {
                state.events.push(InputMethodEvent::Done);
            }
            zwp_input_method_v2::Event::Unavailable => {
                state.events.push(InputMethodEvent::Unavailable);
            }
            _ => {}
        }
    }
}

// Keyboard grab dispatch
impl Dispatch<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2, ()>
    for InputMethodData
{
    fn event(
        _state: &mut Self,
        _grab: &zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
        _event: zwp_input_method_keyboard_grab_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Handle keyboard events if needed
    }
}

// Popup surface dispatch
impl Dispatch<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, ()> for InputMethodData {
    fn event(
        state: &mut Self,
        _popup: &zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            state.events.push(InputMethodEvent::PopupSurfaceCreated {
                x,
                y,
                width,
                height,
            });
        }
    }
}
