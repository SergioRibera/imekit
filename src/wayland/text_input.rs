//! Wayland Text Input v3 implementation
//!
//! This module implements the `zwp_text_input_v3` protocol for Wayland.
//! The protocol allows applications to receive text input from an IME.
//!
//! This is the client-side of the text input protocol, used by applications
//! that want to receive input from an IME.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use wayland_client::{
    protocol::{wl_registry, wl_seat, wl_surface},
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};

use crate::{ContentHint, ContentPurpose, Error, Result};

/// Events from text input
#[derive(Debug, Clone)]
pub enum TextInputEvent {
    /// Enter - text input is now active for a surface
    Enter,
    /// Leave - text input is no longer active
    Leave,
    /// Preedit string received
    PreeditString {
        /// The preedit text
        text: Option<String>,
        /// Cursor begin position
        cursor_begin: i32,
        /// Cursor end position
        cursor_end: i32,
    },
    /// Commit string received
    CommitString {
        /// The text to commit
        text: Option<String>,
    },
    /// Delete surrounding text
    DeleteSurroundingText {
        /// Bytes before cursor to delete
        before_length: u32,
        /// Bytes after cursor to delete
        after_length: u32,
    },
    /// Done - all pending events have been sent
    Done {
        /// Serial number
        serial: u32,
    },
}

/// Text input client for receiving IME input
pub struct TextInput {
    connection: Connection,
    event_queue: EventQueue<TextInputData>,
    data: Arc<Mutex<TextInputData>>,
}

/// Internal data for the text input
#[allow(dead_code)]
#[derive(Default)]
pub struct TextInputData {
    /// Text input manager
    manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    /// Seat
    seat: Option<wl_seat::WlSeat>,
    /// The text input instance
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    /// Current surface
    surface: Option<wl_surface::WlSurface>,
    /// Whether text input is enabled
    enabled: bool,
    /// Pending events
    events: VecDeque<TextInputEvent>,
    /// Current serial
    serial: u32,
}

impl TextInput {
    /// Create a new text input client
    pub fn new() -> Result<Self> {
        let connection = Connection::connect_to_env()?;
        let display = connection.display();

        let data = Arc::new(Mutex::new(TextInputData::default()));
        let mut event_queue = connection.new_event_queue();
        let qh = event_queue.handle();

        // Get the registry and enumerate globals
        display.get_registry(&qh, ());

        // Roundtrip to get globals
        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        // Check if we got the manager
        {
            let data = data.lock().unwrap();
            if data.manager.is_none() {
                return Err(Error::ProtocolNotSupported(
                    "zwp_text_input_manager_v3 not available".to_string(),
                ));
            }
            if data.seat.is_none() {
                return Err(Error::ConnectionFailed("No seat found".to_string()));
            }
        }

        // Create the text input
        {
            let mut data_guard = data.lock().unwrap();
            if let (Some(manager), Some(seat)) = (&data_guard.manager, &data_guard.seat) {
                let ti = manager.get_text_input(seat, &qh, ());
                data_guard.text_input = Some(ti);
            }
        }

        // Another roundtrip
        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        Ok(Self {
            connection,
            event_queue,
            data,
        })
    }

    /// Get the next event
    pub fn next_event(&mut self) -> Option<TextInputEvent> {
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
        self.data.lock().unwrap().events.pop_front()
    }

    /// Enable text input for a surface
    pub fn enable(&mut self) {
        let data = self.data.lock().unwrap();
        if let Some(ti) = &data.text_input {
            ti.enable();
        }
    }

    /// Disable text input
    pub fn disable(&mut self) {
        let data = self.data.lock().unwrap();
        if let Some(ti) = &data.text_input {
            ti.disable();
        }
    }

    /// Set the surrounding text
    pub fn set_surrounding_text(&self, text: &str, cursor: i32, anchor: i32) {
        let data = self.data.lock().unwrap();
        if let Some(ti) = &data.text_input {
            ti.set_surrounding_text(text.to_string(), cursor, anchor);
        }
    }

    /// Set the cursor rectangle
    pub fn set_cursor_rectangle(&self, x: i32, y: i32, width: i32, height: i32) {
        let data = self.data.lock().unwrap();
        if let Some(ti) = &data.text_input {
            ti.set_cursor_rectangle(x, y, width, height);
        }
    }

    /// Set the content type
    pub fn set_content_type(&self, hint: ContentHint, purpose: ContentPurpose) {
        let data = self.data.lock().unwrap();
        if let Some(ti) = &data.text_input {
            let hint_raw = content_hint_to_raw(hint);
            let purpose_wl = content_purpose_to_wl(purpose);
            ti.set_content_type(
                zwp_text_input_v3::ContentHint::from_bits_truncate(hint_raw),
                purpose_wl,
            );
        }
    }

    /// Commit changes
    pub fn commit(&self) {
        let data = self.data.lock().unwrap();
        if let Some(ti) = &data.text_input {
            ti.commit();
        }
    }
}

fn content_hint_to_raw(hint: ContentHint) -> u32 {
    let mut raw = 0u32;
    if hint.completion {
        raw |= 0x1;
    }
    if hint.spellcheck {
        raw |= 0x2;
    }
    if hint.auto_capitalization {
        raw |= 0x4;
    }
    if hint.lowercase {
        raw |= 0x8;
    }
    if hint.uppercase {
        raw |= 0x10;
    }
    if hint.titlecase {
        raw |= 0x20;
    }
    if hint.hidden_text {
        raw |= 0x40;
    }
    if hint.sensitive_data {
        raw |= 0x80;
    }
    if hint.latin {
        raw |= 0x100;
    }
    if hint.multiline {
        raw |= 0x200;
    }
    raw
}

fn content_purpose_to_wl(purpose: ContentPurpose) -> zwp_text_input_v3::ContentPurpose {
    use zwp_text_input_v3::ContentPurpose as WlPurpose;
    match purpose {
        ContentPurpose::Normal => WlPurpose::Normal,
        ContentPurpose::Alpha => WlPurpose::Alpha,
        ContentPurpose::Digits => WlPurpose::Digits,
        ContentPurpose::Number => WlPurpose::Number,
        ContentPurpose::Phone => WlPurpose::Phone,
        ContentPurpose::Url => WlPurpose::Url,
        ContentPurpose::Email => WlPurpose::Email,
        ContentPurpose::Name => WlPurpose::Name,
        ContentPurpose::Password => WlPurpose::Password,
        ContentPurpose::Pin => WlPurpose::Pin,
        ContentPurpose::Date => WlPurpose::Date,
        ContentPurpose::Time => WlPurpose::Time,
        ContentPurpose::Datetime => WlPurpose::Datetime,
        ContentPurpose::Terminal => WlPurpose::Terminal,
    }
}

// Registry dispatch
impl Dispatch<wl_registry::WlRegistry, ()> for TextInputData {
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
                "zwp_text_input_manager_v3" => {
                    let manager = registry
                        .bind::<zwp_text_input_manager_v3::ZwpTextInputManagerV3, _, _>(
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
impl Dispatch<wl_seat::WlSeat, ()> for TextInputData {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No-op
    }
}

// Text input manager dispatch
impl Dispatch<zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()> for TextInputData {
    fn event(
        _state: &mut Self,
        _manager: &zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        _event: zwp_text_input_manager_v3::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No events
    }
}

// Text input dispatch
impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for TextInputData {
    fn event(
        state: &mut Self,
        _ti: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { surface: _ } => {
                state.enabled = true;
                state.events.push_back(TextInputEvent::Enter);
            }
            zwp_text_input_v3::Event::Leave { surface: _ } => {
                state.enabled = false;
                state.events.push_back(TextInputEvent::Leave);
            }
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                state.events.push_back(TextInputEvent::PreeditString {
                    text,
                    cursor_begin,
                    cursor_end,
                });
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                state.events.push_back(TextInputEvent::CommitString { text });
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                state.events.push_back(TextInputEvent::DeleteSurroundingText {
                    before_length,
                    after_length,
                });
            }
            zwp_text_input_v3::Event::Done { serial } => {
                state.serial = serial;
                state.events.push_back(TextInputEvent::Done { serial });
            }
            _ => {}
        }
    }
}
