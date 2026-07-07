//! Wayland Input Method v2 implementation
//!
//! This module implements the `zwp_input_method_v2` protocol for Wayland.
//! The protocol allows applications to act as input methods.

use std::collections::VecDeque;
use std::os::unix::io::{AsFd, AsRawFd, RawFd};
use std::sync::{Arc, Mutex};

use wayland_client::{
    protocol::{wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_surface},
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2, zwp_input_method_manager_v2, zwp_input_method_v2,
    zwp_input_popup_surface_v2,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use xkbcommon::xkb;

use crate::{
    ChangeCause, ContentHint, ContentPurpose, Error, InputMethodEvent, InputMethodState, KeyState,
    Modifiers, Result,
};

// SAFETY: xkbcommon types wrap raw C pointers but libxkbcommon does not share
// objects across threads. InputMethodData lives behind Arc<Mutex<>>, so only one
// thread accesses it at a time — satisfying xkbcommon's single-owner contract.
struct XkbWrapper<T>(T);
unsafe impl<T> Send for XkbWrapper<T> {}

/// Wayland input method implementation using zwp_input_method_v2
pub struct InputMethod {
    pub(super) connection: Connection,
    event_queue: EventQueue<InputMethodData>,
    data: Arc<Mutex<InputMethodData>>,
}

/// A popup surface for rendering candidate windows.
///
/// Obtained via [`InputMethod::create_popup_surface`]. The compositor sends
/// [`InputMethodEvent::PopupSurfaceCreated`] events carrying the position.
/// Drop to destroy the popup.
pub struct PopupSurface {
    surface: wl_surface::WlSurface,
    _popup: zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
}

impl PopupSurface {
    /// The underlying `wl_surface` — render candidate UI into this.
    pub fn surface(&self) -> &wl_surface::WlSurface {
        &self.surface
    }
}

/// Internal data for the input method
#[allow(dead_code)]
pub struct InputMethodData {
    manager: Option<zwp_input_method_manager_v2::ZwpInputMethodManagerV2>,
    seats: Vec<(wl_seat::WlSeat, Option<String>)>,
    input_method: Option<zwp_input_method_v2::ZwpInputMethodV2>,
    keyboard_grab: Option<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2>,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
    done_serial: u32,
    pending_activate: bool,
    unavailable: bool,
    // compositor for popup surface creation
    compositor: Option<wl_compositor::WlCompositor>,
    // xkb state for keyboard grab keysym resolution
    xkb_ctx: XkbWrapper<xkb::Context>,
    xkb_state: Option<XkbWrapper<xkb::State>>,
    current_modifiers: Modifiers,
    last_key_time: u32,
    // virtual keyboard for forwarding grabbed keys back to apps
    virtual_keyboard_manager: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,
    virtual_keyboard: Option<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1>,
    keymap_sent_to_vkb: bool,
}

impl Default for InputMethodData {
    fn default() -> Self {
        Self {
            manager: None,
            seats: Vec::new(),
            input_method: None,
            keyboard_grab: None,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            done_serial: 0,
            pending_activate: false,
            unavailable: false,
            compositor: None,
            xkb_ctx: XkbWrapper(xkb::Context::new(0)),
            xkb_state: None,
            current_modifiers: Modifiers::default(),
            last_key_time: 0,
            virtual_keyboard_manager: None,
            virtual_keyboard: None,
            keymap_sent_to_vkb: false,
        }
    }
}

/// Clone-able, `Send`-safe handle for cross-thread text commits.
///
/// Obtain via [`InputMethod::handle`]. This type can be sent to other
/// threads; the event-receiving [`InputMethod`] stays on its original thread.
#[derive(Clone)]
pub struct InputMethodHandle {
    data: Arc<Mutex<InputMethodData>>,
    connection: Connection,
}

impl InputMethodHandle {
    pub fn commit_string(&self, text: &str) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.commit_string(text.to_string());
            Ok(())
        } else {
            Err(Error::NotActive)
        }
    }

    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, cursor_end: i32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.set_preedit_string(text.to_string(), cursor_begin, cursor_end);
            Ok(())
        } else {
            Err(Error::NotActive)
        }
    }

    pub fn delete_surrounding_text(&self, before_length: u32, after_length: u32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.delete_surrounding_text(before_length, after_length);
            Ok(())
        } else {
            Err(Error::NotActive)
        }
    }

    pub fn commit(&self, serial: u32) -> Result<()> {
        {
            let data = self.data.lock().unwrap();
            if let Some(im) = &data.input_method {
                im.commit(serial);
            } else {
                return Err(Error::NotActive);
            }
        }
        self.connection
            .flush()
            .map_err(|e| Error::CommitFailed(e.to_string()))
    }
}

impl InputMethod {
    /// Create a new input method instance connecting to the default Wayland display.
    pub fn new() -> Result<Self> {
        let connection = Connection::connect_to_env()?;
        let display = connection.display();

        let data = Arc::new(Mutex::new(InputMethodData::default()));
        let mut event_queue = connection.new_event_queue();
        let qh = event_queue.handle();

        display.get_registry(&qh, ());

        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        {
            let data = data.lock().unwrap();
            if data.manager.is_none() {
                return Err(Error::ProtocolNotSupported(
                    "zwp_input_method_manager_v2 not available".to_string(),
                ));
            }
            if data.seats.is_empty() {
                return Err(Error::ConnectionFailed("No seat found".to_string()));
            }
        }

        {
            let mut d = data.lock().unwrap();
            let seat = d.seats.first().map(|(s, _)| s.clone());
            let manager = d.manager.clone();
            let vkm = d.virtual_keyboard_manager.clone();

            if let (Some(m), Some(s)) = (manager, seat.clone()) {
                d.input_method = Some(m.get_input_method(&s, &qh, ()));
            }
            if let (Some(m), Some(s)) = (vkm, seat) {
                d.virtual_keyboard = Some(m.create_virtual_keyboard(&s, &qh, ()));
            }
        }

        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        Ok(Self { connection, event_queue, data })
    }

    /// Create an input method bound to a specific seat (for multi-seat setups).
    pub fn new_for_seat(seat_name: &str) -> Result<Self> {
        let connection = Connection::connect_to_env()?;
        let display = connection.display();

        let data = Arc::new(Mutex::new(InputMethodData::default()));
        let mut event_queue = connection.new_event_queue();
        let qh = event_queue.handle();

        display.get_registry(&qh, ());

        // Two roundtrips: first binds globals, second receives seat names.
        event_queue.roundtrip(&mut *data.lock().unwrap())?;
        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        {
            let data = data.lock().unwrap();
            if data.manager.is_none() {
                return Err(Error::ProtocolNotSupported(
                    "zwp_input_method_manager_v2 not available".to_string(),
                ));
            }
        }

        {
            let mut d = data.lock().unwrap();
            let seat = d
                .seats
                .iter()
                .find(|(_, name)| name.as_deref() == Some(seat_name))
                .map(|(s, _)| s.clone())
                .ok_or_else(|| {
                    Error::ConnectionFailed(format!("seat '{}' not found", seat_name))
                })?;
            let manager = d.manager.clone();
            let vkm = d.virtual_keyboard_manager.clone();

            if let Some(m) = manager {
                d.input_method = Some(m.get_input_method(&seat, &qh, ()));
            }
            if let Some(m) = vkm {
                d.virtual_keyboard = Some(m.create_virtual_keyboard(&seat, &qh, ()));
            }
        }

        event_queue.roundtrip(&mut *data.lock().unwrap())?;

        Ok(Self { connection, event_queue, data })
    }

    /// Get the next event from the input method without blocking.
    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        let _ = self
            .event_queue
            .dispatch_pending(&mut *self.data.lock().unwrap());

        if let Some(guard) = self.connection.prepare_read() {
            if let Err(e) = guard.read() {
                log_warn!("Wayland connection read error: {}", e);
            }
        }
        let _ = self
            .event_queue
            .dispatch_pending(&mut *self.data.lock().unwrap());

        self.data.lock().unwrap().events.pop_front()
    }

    /// Dispatch events (blocking).
    pub fn dispatch(&mut self) -> Result<()> {
        self.event_queue
            .blocking_dispatch(&mut *self.data.lock().unwrap())?;
        Ok(())
    }

    /// Raw file descriptor of the Wayland socket.
    ///
    /// Register this in your own poller (epoll, kqueue, etc.) to drive the
    /// input method without polling. When it becomes readable, call
    /// [`next_event`](Self::next_event) or [`dispatch`](Self::dispatch).
    pub fn as_raw_fd(&self) -> RawFd {
        self.connection.as_fd().as_raw_fd()
    }

    /// Create a popup surface for rendering candidate windows.
    ///
    /// The returned [`PopupSurface`] owns the `wl_surface`; drop it to
    /// destroy the popup. The compositor will start sending
    /// [`InputMethodEvent::PopupSurfaceCreated`] events with position info.
    pub fn create_popup_surface(&mut self) -> Result<PopupSurface> {
        let qh = self.event_queue.handle();
        let d = self.data.lock().unwrap();

        let compositor = d
            .compositor
            .clone()
            .ok_or_else(|| Error::ProtocolNotSupported("wl_compositor not available".to_string()))?;
        let im = d
            .input_method
            .clone()
            .ok_or(Error::NotActive)?;

        drop(d);

        let surface = compositor.create_surface(&qh, ());
        let popup = im.get_input_popup_surface(&surface, &qh, ());

        Ok(PopupSurface { surface, _popup: popup })
    }

    /// Grab the keyboard — key events are then delivered as
    /// [`InputMethodEvent::KeyEvent`].
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

    /// Forward a grabbed key back to the focused application via virtual keyboard.
    ///
    /// Requires a previous call to [`grab_keyboard`](Self::grab_keyboard) and
    /// compositor support for `zwp_virtual_keyboard_manager_v1`. Uses the
    /// timestamp from the most recent key event.
    pub fn forward_key(&self, keycode: u32, state: KeyState) -> Result<()> {
        let data = self.data.lock().unwrap();
        let vk = data.virtual_keyboard.as_ref().ok_or(Error::NotActive)?;
        if !data.keymap_sent_to_vkb {
            return Err(Error::NotActive);
        }
        let state_u32 = match state {
            KeyState::Pressed => 1u32,
            KeyState::Released => 0u32,
        };
        vk.key(data.last_key_time, keycode, state_u32);
        drop(data);
        self.connection
            .flush()
            .map_err(|e| Error::CommitFailed(e.to_string()))
    }

    /// Check if the input method is active.
    pub fn is_active(&self) -> bool {
        self.data.lock().unwrap().state.active
    }

    /// Commit text to the client.
    pub fn commit_string(&self, text: &str) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.commit_string(text.to_string());
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Set preedit text (composing text shown to user).
    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, cursor_end: i32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.set_preedit_string(text.to_string(), cursor_begin, cursor_end);
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Delete surrounding text.
    pub fn delete_surrounding_text(&self, before_length: u32, after_length: u32) -> Result<()> {
        let data = self.data.lock().unwrap();
        if let Some(im) = &data.input_method {
            im.delete_surrounding_text(before_length, after_length);
            return Ok(());
        }
        Err(Error::NotActive)
    }

    /// Commit all pending changes.
    pub fn commit(&self, serial: u32) -> Result<()> {
        {
            let data = self.data.lock().unwrap();
            if let Some(im) = &data.input_method {
                im.commit(serial);
            } else {
                return Err(Error::NotActive);
            }
        }
        self.connection
            .flush()
            .map_err(|e| Error::CommitFailed(e.to_string()))
    }

    /// Get the current state.
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

    /// Returns true if the compositor has withdrawn this input method.
    pub fn is_unavailable(&self) -> bool {
        self.data.lock().unwrap().unavailable
    }

    /// Returns the current operational status.
    pub fn status(&self) -> crate::Status {
        let data = self.data.lock().unwrap();
        if data.unavailable {
            crate::Status::Unavailable
        } else if data.state.active {
            crate::Status::Active
        } else {
            crate::Status::Inactive
        }
    }

    /// Convert into an async [`InputMethodStream`].
    ///
    /// Requires the `async` feature. The stream yields events as they arrive
    /// from the compositor without blocking the executor thread.
    #[cfg(feature = "async")]
    pub fn into_stream(self) -> std::io::Result<super::async_stream::InputMethodStream> {
        super::async_stream::InputMethodStream::new(self)
    }

    /// Get a clone-able handle for sending commits from other threads.
    pub fn handle(&self) -> InputMethodHandle {
        InputMethodHandle {
            data: Arc::clone(&self.data),
            connection: self.connection.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper parsers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Dispatch impls
// ---------------------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for InputMethodData {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "zwp_input_method_manager_v2" => {
                    state.manager = Some(
                        registry.bind::<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ),
                    );
                }
                "wl_seat" => {
                    let seat =
                        registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(8), qh, ());
                    state.seats.push((seat, None));
                }
                "wl_compositor" => {
                    state.compositor = Some(
                        registry.bind::<wl_compositor::WlCompositor, _, _>(
                            name,
                            version.min(5),
                            qh,
                            (),
                        ),
                    );
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.virtual_keyboard_manager = Some(
                        registry.bind::<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for InputMethodData {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Name { name } = event {
            if let Some(entry) = state.seats.iter_mut().find(|(s, _)| s == seat) {
                entry.1 = Some(name);
            }
        }
    }
}

impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for InputMethodData {
    fn event(
        _: &mut Self,
        _: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for InputMethodData {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for InputMethodData {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, ()>
    for InputMethodData
{
    fn event(
        _: &mut Self,
        _: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        _: zwp_virtual_keyboard_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, ()> for InputMethodData {
    fn event(
        _: &mut Self,
        _: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
        _: zwp_virtual_keyboard_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

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
                state.pending_activate = true;
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.state.active = false;
                state.pending_activate = false;
                state.state.reset();
                state.events.push_back(InputMethodEvent::Deactivate);
            }
            zwp_input_method_v2::Event::SurroundingText { text, cursor, anchor } => {
                state.state.surrounding_text = Some(text.clone());
                state.state.cursor = cursor;
                state.state.anchor = anchor;
                state.events.push_back(InputMethodEvent::SurroundingText { text, cursor, anchor });
            }
            zwp_input_method_v2::Event::TextChangeCause { cause } => {
                use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ChangeCause as WlChangeCause;
                let cause = match cause {
                    WEnum::Value(WlChangeCause::InputMethod) => ChangeCause::InputMethod,
                    _ => ChangeCause::Other,
                };
                state.state.change_cause = cause;
                state.events.push_back(InputMethodEvent::TextChangeCause(cause));
            }
            zwp_input_method_v2::Event::ContentType { hint, purpose } => {
                let hint_raw = match hint {
                    WEnum::Value(h) => h.bits(),
                    WEnum::Unknown(v) => v,
                };
                let content_hint = parse_content_hint(hint_raw);
                let content_purpose = parse_content_purpose(purpose);
                state.state.content_hint = content_hint;
                state.state.content_purpose = content_purpose;
                state.events.push_back(InputMethodEvent::ContentType {
                    hint: content_hint,
                    purpose: content_purpose,
                });
            }
            zwp_input_method_v2::Event::Done => {
                state.done_serial += 1;
                state.state.serial = state.done_serial;
                if state.pending_activate {
                    state.pending_activate = false;
                    state
                        .events
                        .push_back(InputMethodEvent::Activate { serial: state.done_serial });
                }
                state.events.push_back(InputMethodEvent::Done);
            }
            zwp_input_method_v2::Event::Unavailable => {
                state.unavailable = true;
                state.events.push_back(InputMethodEvent::Unavailable);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2, ()>
    for InputMethodData
{
    fn event(
        state: &mut Self,
        _grab: &zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
        event: zwp_input_method_keyboard_grab_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_keyboard_grab_v2::Event::Keymap { format, fd, size } => {
                // Dup fd before consuming it so the same keymap can be forwarded
                // to the virtual keyboard for transparent key forwarding.
                let vkb_fd = if state.virtual_keyboard.is_some() {
                    fd.as_fd().try_clone_to_owned().ok()
                } else {
                    None
                };

                if matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    match unsafe {
                        xkb::Keymap::new_from_fd(
                            &state.xkb_ctx.0,
                            fd,
                            size as usize,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                    } {
                        Ok(Some(keymap)) => {
                            state.xkb_state = Some(XkbWrapper(xkb::State::new(&keymap)));
                        }
                        _ => {
                            log_warn!("Failed to parse XKB keymap from compositor");
                        }
                    }
                }

                if let (Some(vk), Some(vkb_fd)) = (&state.virtual_keyboard, vkb_fd) {
                    // 1 = XKB_KEYMAP_FORMAT_TEXT_V1
                    vk.keymap(1, vkb_fd.as_fd(), size);
                    state.keymap_sent_to_vkb = true;
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::Key { serial: _, time, key, state: key_state } => {
                state.last_key_time = time;

                if let Some(xkb_state) = &state.xkb_state {
                    // Wayland keycodes are evdev codes; XKB adds 8.
                    let keycode = xkb::Keycode::new(key + 8);
                    let keysym = xkb_state.0.key_get_one_sym(keycode).raw();

                    let ks = match key_state {
                        WEnum::Value(wl_keyboard::KeyState::Pressed) => KeyState::Pressed,
                        _ => KeyState::Released,
                    };

                    state.events.push_back(InputMethodEvent::KeyEvent {
                        keycode: key,
                        keysym,
                        state: ks,
                        modifiers: state.current_modifiers,
                    });
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if let Some(xkb_state) = &mut state.xkb_state {
                    xkb_state.0.update_mask(
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        0,
                        0,
                        group,
                    );
                    let s = &xkb_state.0;
                    state.current_modifiers = Modifiers {
                        shift: s.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE),
                        caps: s.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE),
                        ctrl: s.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE),
                        alt: s.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE),
                        logo: s.mod_name_is_active(xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE),
                        num: s.mod_name_is_active(xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE),
                    };
                }

                if let Some(vk) = &state.virtual_keyboard {
                    if state.keymap_sent_to_vkb {
                        vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
                    }
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::RepeatInfo { rate, delay } => {
                state.events.push_back(InputMethodEvent::RepeatInfo { rate, delay });
            }

            _ => {}
        }
    }
}

impl Dispatch<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, ()> for InputMethodData {
    fn event(
        state: &mut Self,
        _popup: &zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle { x, y, width, height } =
            event
        {
            state
                .events
                .push_back(InputMethodEvent::PopupSurfaceCreated { x, y, width, height });
        }
    }
}
