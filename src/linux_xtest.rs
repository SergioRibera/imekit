//! Shared XTest text injection for Linux backends (X11 and IBus)

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::{Error, Result};

const XK_SHIFT_L: Keysym = 0xFFE1;
const XK_CONTROL_L: Keysym = 0xFFE3;
const XK_BACKSPACE: Keysym = 0xFF08;
const XK_DELETE: Keysym = 0xFFFF;
const XK_U: Keysym = 0x0075;
const XK_SPACE: Keysym = 0x0020;

pub struct CachedKeyboardMapping {
    pub keysyms: Vec<Keysym>,
    pub keysyms_per_keycode: usize,
    pub min_keycode: Keycode,
}

pub fn load_keyboard_mapping(conn: &RustConnection) -> Result<CachedKeyboardMapping> {
    let min_keycode = conn.setup().min_keycode;
    let max_keycode = conn.setup().max_keycode;
    let mapping = conn
        .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)
        .map_err(|e| Error::ConnectionFailed(e.to_string()))?
        .reply()
        .map_err(|e| Error::ConnectionFailed(e.to_string()))?;
    Ok(CachedKeyboardMapping {
        keysyms: mapping.keysyms,
        keysyms_per_keycode: mapping.keysyms_per_keycode as usize,
        min_keycode,
    })
}

pub fn commit_string(
    conn: &RustConnection,
    root: Window,
    mapping: &CachedKeyboardMapping,
    text: &str,
) -> Result<()> {
    for c in text.chars() {
        let keysym = char_to_keysym(c);
        if keysym == 0 {
            log_warn!("No keysym for character: {:?}", c);
            continue;
        }
        match find_keycode(keysym, mapping) {
            Some((kc, shift)) => send_key(conn, root, mapping, kc, shift)?,
            None => send_unicode(conn, root, mapping, c)?,
        }
    }
    conn.flush()
        .map_err(|e| Error::CommitFailed(e.to_string()))
}

pub fn delete_surrounding_text(
    conn: &RustConnection,
    root: Window,
    mapping: &CachedKeyboardMapping,
    before: u32,
    after: u32,
) -> Result<()> {
    let bs = find_keycode(XK_BACKSPACE, mapping).map(|(kc, _)| kc).unwrap_or(22);
    let del = find_keycode(XK_DELETE, mapping).map(|(kc, _)| kc).unwrap_or(119);
    for _ in 0..before {
        send_key(conn, root, mapping, bs, false)?;
    }
    for _ in 0..after {
        send_key(conn, root, mapping, del, false)?;
    }
    conn.flush()
        .map_err(|e| Error::CommitFailed(e.to_string()))
}

pub fn char_to_keysym(c: char) -> Keysym {
    let code = c as u32;
    if (0x20..=0x7E).contains(&code) {
        return code;
    }
    if (0xA0..=0xFF).contains(&code) {
        return code;
    }
    if code > 0xFF {
        return 0x0100_0000 | code;
    }
    0
}

/// Self-contained XTest writer for backends that need their own X11 connection (e.g. IBus).
#[cfg(feature = "ibus")]
pub struct XTestWriter {
    connection: RustConnection,
    root_window: Window,
    mapping: CachedKeyboardMapping,
}

#[cfg(feature = "ibus")]
impl XTestWriter {
    pub fn new() -> Result<Self> {
        let (conn, screen_num) = RustConnection::connect(None)
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let xtest_ext = conn
            .query_extension(b"XTEST")
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        if !xtest_ext.present {
            return Err(Error::ProtocolNotSupported(
                "XTest extension not available".to_string(),
            ));
        }

        xtest::get_version(&conn, 2, 2)
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?
            .reply()
            .map_err(|e| Error::ConnectionFailed(e.to_string()))?;

        let root = conn.setup().roots[screen_num].root;
        let mapping = load_keyboard_mapping(&conn)?;

        Ok(Self { connection: conn, root_window: root, mapping })
    }

    pub fn commit_string(&self, text: &str) -> Result<()> {
        commit_string(&self.connection, self.root_window, &self.mapping, text)
    }

    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<()> {
        delete_surrounding_text(&self.connection, self.root_window, &self.mapping, before, after)
    }
}

fn find_keycode(keysym: Keysym, mapping: &CachedKeyboardMapping) -> Option<(Keycode, bool)> {
    for (i, chunk) in mapping.keysyms.chunks(mapping.keysyms_per_keycode).enumerate() {
        let keycode = mapping.min_keycode + i as u8;
        if !chunk.is_empty() && chunk[0] == keysym {
            return Some((keycode, false));
        }
        if chunk.len() > 1 && chunk[1] == keysym {
            return Some((keycode, true));
        }
    }
    None
}

fn fake_key(conn: &RustConnection, event_type: u8, keycode: Keycode, root: Window) -> Result<()> {
    xtest::fake_input(conn, event_type, keycode, x11rb::CURRENT_TIME, root, 0, 0, 0)
        .map_err(|e| Error::CommitFailed(e.to_string()))?;
    Ok(())
}

fn send_key(
    conn: &RustConnection,
    root: Window,
    mapping: &CachedKeyboardMapping,
    keycode: Keycode,
    needs_shift: bool,
) -> Result<()> {
    let shift = find_keycode(XK_SHIFT_L, mapping).map(|(kc, _)| kc).unwrap_or(50);
    if needs_shift {
        fake_key(conn, KEY_PRESS_EVENT, shift, root)?;
    }
    fake_key(conn, KEY_PRESS_EVENT, keycode, root)?;
    fake_key(conn, KEY_RELEASE_EVENT, keycode, root)?;
    if needs_shift {
        fake_key(conn, KEY_RELEASE_EVENT, shift, root)?;
    }
    Ok(())
}

fn send_unicode(
    conn: &RustConnection,
    root: Window,
    mapping: &CachedKeyboardMapping,
    c: char,
) -> Result<()> {
    let hex = format!("{:x}", c as u32);

    let ctrl = find_keycode(XK_CONTROL_L, mapping).map(|(kc, _)| kc).unwrap_or(37);
    let shift = find_keycode(XK_SHIFT_L, mapping).map(|(kc, _)| kc).unwrap_or(50);
    let u = find_keycode(XK_U, mapping).map(|(kc, _)| kc).unwrap_or(30);
    let space = find_keycode(XK_SPACE, mapping).map(|(kc, _)| kc).unwrap_or(65);

    fake_key(conn, KEY_PRESS_EVENT, ctrl, root)?;
    fake_key(conn, KEY_PRESS_EVENT, shift, root)?;
    fake_key(conn, KEY_PRESS_EVENT, u, root)?;
    fake_key(conn, KEY_RELEASE_EVENT, u, root)?;
    fake_key(conn, KEY_RELEASE_EVENT, shift, root)?;
    fake_key(conn, KEY_RELEASE_EVENT, ctrl, root)?;

    for h in hex.chars() {
        let ks = char_to_keysym(h);
        if let Some((kc, sh)) = find_keycode(ks, mapping) {
            send_key(conn, root, mapping, kc, sh)?;
        }
    }

    fake_key(conn, KEY_PRESS_EVENT, space, root)?;
    fake_key(conn, KEY_RELEASE_EVENT, space, root)?;

    Ok(())
}
