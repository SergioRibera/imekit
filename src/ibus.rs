//! IBus Input Method — proper engine registration with XTest fallback
//!
//! Registers as `org.freedesktop.IBus.Engine` so IBus can route key events
//! to imekit and receive `CommitText` / `UpdatePreeditText` signals back.
//!
//! Falls back to client-mode + XTest injection when engine registration
//! fails (e.g. IBus not running, name already taken).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use zbus::{
    blocking::{connection::Builder, Connection},
    zvariant::{Array, Dict, OwnedValue, Signature, StructureBuilder, Value},
};

use crate::{linux_xtest, Error, InputMethodEvent, InputMethodState, Result};

const IBUS_SERVICE: &str = "org.freedesktop.IBus";
const IBUS_PATH: &str = "/org/freedesktop/IBus";
const IBUS_INTERFACE: &str = "org.freedesktop.IBus";
const IBUS_INPUT_CONTEXT_INTERFACE: &str = "org.freedesktop.IBus.InputContext";
const IBUS_ENGINE_BUS_NAME: &str = "org.freedesktop.IBus.Engine.imekit";
const IBUS_ENGINE_PATH: &str = "/org/freedesktop/IBus/Engine/imekit";
const IBUS_ENGINE_INTERFACE: &str = "org.freedesktop.IBus.Engine";

// ── IBus wire type helpers ───────────────────────────────────────────────────

fn empty_dict_sv() -> Dict<'static, 'static> {
    Dict::new(&Signature::Str, &Signature::Variant)
}

fn empty_array_v() -> Array<'static> {
    Array::new(&Signature::Variant)
}

/// Build an `OwnedValue` wrapping an IBusAttrList `(sa{sv}av)`.
fn make_ibus_attr_list() -> zbus::zvariant::Result<OwnedValue> {
    let s = StructureBuilder::new()
        .add_field("IBusAttrList".to_string())
        .add_field(Value::from(empty_dict_sv()))
        .add_field(empty_array_v())
        .build()?;
    OwnedValue::try_from(Value::from(s))
}

/// Build an `OwnedValue` wrapping an IBusText `(sa{sv}sv)`.
fn make_ibus_text(text: &str) -> zbus::zvariant::Result<OwnedValue> {
    let attr_list = make_ibus_attr_list()?;
    let s = StructureBuilder::new()
        .add_field("IBusText".to_string())
        .add_field(Value::from(empty_dict_sv()))
        .add_field(text.to_string())
        .add_field(attr_list)
        .build()?;
    OwnedValue::try_from(Value::from(s))
}

/// Build an `OwnedValue` wrapping an IBusEngineDesc `(sa{sv}sssssssuussssss)`.
fn make_ibus_engine_desc() -> zbus::zvariant::Result<OwnedValue> {
    let s = StructureBuilder::new()
        .add_field("IBusEngineDesc".to_string())
        .add_field(Value::from(empty_dict_sv()))
        .add_field("imekit".to_string())      // name
        .add_field("imekit".to_string())      // longname
        .add_field("imekit input method engine".to_string())
        .add_field("".to_string())            // language
        .add_field("MIT".to_string())         // license
        .add_field("".to_string())            // author
        .add_field("".to_string())            // icon
        .add_field("default".to_string())     // layout
        .add_field(0u32)                      // rank
        .add_field("".to_string())            // hotkeys
        .add_field("IM".to_string())          // symbol
        .add_field("".to_string())            // setup
        .add_field("".to_string())            // layout_variant
        .add_field("".to_string())            // layout_option
        .add_field("0.1.1".to_string())       // version
        .add_field("imekit".to_string())      // textdomain
        .build()?;
    OwnedValue::try_from(Value::from(s))
}

/// Build an `OwnedValue` wrapping an IBusComponent `(sa{sv}ssssssssavav)`.
fn make_ibus_component() -> zbus::zvariant::Result<OwnedValue> {
    let engine_owned = make_ibus_engine_desc()?;

    // engines array: av, one entry
    let mut engines_arr = Array::new(&Signature::Variant);
    engines_arr.append(Value::from(engine_owned))?;

    let s = StructureBuilder::new()
        .add_field("IBusComponent".to_string())
        .add_field(Value::from(empty_dict_sv()))
        .add_field("org.freedesktop.IBus.imekit".to_string())
        .add_field("imekit IME component".to_string())
        .add_field("0.1.1".to_string())
        .add_field("MIT".to_string())
        .add_field("".to_string())
        .add_field("".to_string())
        .add_field("".to_string())
        .add_field("imekit".to_string())
        .add_field(empty_array_v())  // observed_paths (av)
        .add_field(engines_arr)      // engines (av)
        .build()?;
    OwnedValue::try_from(Value::from(s))
}

fn register_component(session: &Connection) -> Result<()> {
    let component = make_ibus_component()
        .map_err(|e| Error::IBus(format!("component serialization failed: {e}")))?;

    session
        .call_method(
            Some(IBUS_SERVICE),
            IBUS_PATH,
            Some(IBUS_INTERFACE),
            "RegisterComponent",
            &(component,),
        )
        .map_err(|e| Error::IBus(format!("RegisterComponent failed: {e}")))?;
    Ok(())
}

// ── Engine D-Bus service ─────────────────────────────────────────────────────

struct EngineSharedState {
    events: Mutex<VecDeque<InputMethodEvent>>,
    serial: Mutex<u32>,
    active: Mutex<bool>,
}

struct IBusEngineService {
    state: Arc<EngineSharedState>,
}

#[zbus::interface(name = "org.freedesktop.IBus.Engine")]
impl IBusEngineService {
    fn focus_in(&self) {
        let mut serial = self.state.serial.lock().unwrap();
        *serial += 1;
        let s = *serial;
        *self.state.active.lock().unwrap() = true;
        self.state
            .events
            .lock()
            .unwrap()
            .push_back(InputMethodEvent::Activate { serial: s });
    }

    fn focus_out(&self) {
        *self.state.active.lock().unwrap() = false;
        self.state
            .events
            .lock()
            .unwrap()
            .push_back(InputMethodEvent::Deactivate);
    }

    fn process_key_event(&self, _keyval: u32, _keycode: u32, _state: u32) -> bool {
        false
    }

    fn enable(&self) {}
    fn disable(&self) {}
    fn reset(&self) {}
    fn set_capabilities(&self, _caps: u32) {}
    fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    fn property_activate(&self, _prop_name: &str, _prop_state: u32) {}
    fn page_up(&self) {}
    fn page_down(&self) {}
    fn cursor_up(&self) {}
    fn cursor_down(&self) {}
}

// ── Public InputMethod ───────────────────────────────────────────────────────

pub struct InputMethod {
    session: Connection,
    /// Alive connection where we serve the engine interface (engine mode)
    engine_conn: Option<Connection>,
    /// Shared state with the engine service object
    engine_state: Option<Arc<EngineSharedState>>,
    /// Path of the IBus InputContext we created (client-mode fallback)
    input_context_path: Option<String>,
    xtest: Option<linux_xtest::XTestWriter>,
    state: InputMethodState,
    local_events: VecDeque<InputMethodEvent>,
    serial: u32,
}

impl InputMethod {
    pub fn new() -> Result<Self> {
        let session = Connection::session()
            .map_err(|e| Error::IBus(format!("D-Bus session connection failed: {e}")))?;

        let has_owner: bool = session
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &(IBUS_SERVICE,),
            )
            .map_err(|e| Error::IBus(format!("D-Bus query failed: {e}")))?
            .body()
            .deserialize()
            .map_err(|e| Error::IBus(format!("D-Bus reply parse failed: {e}")))?;

        if !has_owner {
            return Err(Error::IBus("IBus service is not running".into()));
        }

        let xtest = linux_xtest::XTestWriter::new().ok();

        let mut im = Self {
            session,
            engine_conn: None,
            engine_state: None,
            input_context_path: None,
            xtest,
            state: InputMethodState::new(),
            local_events: VecDeque::new(),
            serial: 0,
        };

        match im.try_register_engine() {
            Ok(()) => {
                log_info!("IBus engine registered at {}", IBUS_ENGINE_PATH);
            }
            Err(e) => {
                log_info!(
                    "IBus engine registration failed ({}), using client+XTest fallback",
                    e
                );
                im.create_input_context()?;
            }
        }

        Ok(im)
    }

    fn try_register_engine(&mut self) -> Result<()> {
        let shared = Arc::new(EngineSharedState {
            events: Mutex::new(VecDeque::new()),
            serial: Mutex::new(0),
            active: Mutex::new(false),
        });

        let conn = Builder::session()
            .map_err(|e| Error::IBus(e.to_string()))?
            .name(IBUS_ENGINE_BUS_NAME)
            .map_err(|e| Error::IBus(e.to_string()))?
            .serve_at(
                IBUS_ENGINE_PATH,
                IBusEngineService {
                    state: Arc::clone(&shared),
                },
            )
            .map_err(|e| Error::IBus(e.to_string()))?
            .build()
            .map_err(|e| Error::IBus(e.to_string()))?;

        register_component(&self.session)?;

        self.engine_conn = Some(conn);
        self.engine_state = Some(shared);
        Ok(())
    }

    fn create_input_context(&mut self) -> Result<()> {
        let reply = self
            .session
            .call_method(
                Some(IBUS_SERVICE),
                IBUS_PATH,
                Some(IBUS_INTERFACE),
                "CreateInputContext",
                &("imekit",),
            )
            .map_err(|e| Error::IBus(format!("CreateInputContext failed: {e}")))?;

        let ctx_path: String = reply
            .body()
            .deserialize()
            .map_err(|e| Error::IBus(format!("context path parse failed: {e}")))?;

        log_debug!("IBus input context: {}", ctx_path);
        self.input_context_path = Some(ctx_path);
        self.client_focus_in()
    }

    fn client_focus_in(&mut self) -> Result<()> {
        if let Some(ref path) = self.input_context_path {
            self.session
                .call_method(
                    Some(IBUS_SERVICE),
                    path.as_str(),
                    Some(IBUS_INPUT_CONTEXT_INTERFACE),
                    "FocusIn",
                    &(),
                )
                .map_err(|e| Error::IBus(format!("FocusIn failed: {e}")))?;

            self.state.active = true;
            self.serial += 1;
            self.state.serial = self.serial;
            self.local_events
                .push_back(InputMethodEvent::Activate { serial: self.serial });
        }
        Ok(())
    }

    fn client_focus_out(&mut self) -> Result<()> {
        if let Some(ref path) = self.input_context_path {
            self.session
                .call_method(
                    Some(IBUS_SERVICE),
                    path.as_str(),
                    Some(IBUS_INPUT_CONTEXT_INTERFACE),
                    "FocusOut",
                    &(),
                )
                .map_err(|e| Error::IBus(format!("FocusOut failed: {e}")))?;

            self.state.active = false;
            self.local_events.push_back(InputMethodEvent::Deactivate);
        }
        Ok(())
    }

    pub fn next_event(&mut self) -> Option<InputMethodEvent> {
        if let Some(ref shared) = self.engine_state {
            if let Ok(mut q) = shared.events.lock() {
                if let Some(ev) = q.pop_front() {
                    return Some(ev);
                }
            }
        }
        self.local_events.pop_front()
    }

    pub fn is_active(&self) -> bool {
        if let Some(ref shared) = self.engine_state {
            return *shared.active.lock().unwrap();
        }
        self.state.active
    }

    /// Commit text via IBus `CommitText` signal; falls back to XTest.
    pub fn commit_string(&self, text: &str) -> Result<()> {
        if let Some(ref conn) = self.engine_conn {
            if let Ok(text_owned) = make_ibus_text(text) {
                let result = conn.emit_signal(
                    None::<&str>,
                    IBUS_ENGINE_PATH,
                    IBUS_ENGINE_INTERFACE,
                    "CommitText",
                    &(text_owned,),
                );
                if result.is_ok() {
                    return Ok(());
                }
            }
        }

        self.xtest
            .as_ref()
            .ok_or_else(|| {
                Error::ProtocolNotSupported(
                    "XTest not available; IBus commit requires X11 display".into(),
                )
            })?
            .commit_string(text)
    }

    /// Set preedit via IBus `UpdatePreeditText` signal (engine mode only).
    pub fn set_preedit_string(&self, text: &str, cursor_begin: i32, _cursor_end: i32) -> Result<()> {
        if let Some(ref conn) = self.engine_conn {
            if let Ok(text_owned) = make_ibus_text(text) {
                let cursor = cursor_begin as u32;
                let visible = !text.is_empty();
                let _ = conn.emit_signal(
                    None::<&str>,
                    IBUS_ENGINE_PATH,
                    IBUS_ENGINE_INTERFACE,
                    "UpdatePreeditText",
                    &(text_owned, cursor, visible),
                );
                return Ok(());
            }
        }
        log_debug!("IBus preedit only available in engine mode");
        Ok(())
    }

    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        self.xtest
            .as_ref()
            .ok_or_else(|| {
                Error::ProtocolNotSupported(
                    "XTest not available; delete surrounding text requires X11 display".into(),
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
        let _ = self.client_focus_out();
    }
}
