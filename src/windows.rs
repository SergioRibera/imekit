//! Windows Input Method implementation
//!
//! Commit: TSF `ITfInsertAtSelection` (falls back to `SendInput`).
//! Preedit: TSF `ITfContextComposition` + `ITfRange::SetText` (falls back to IMM32).
//! Events: Win32 message queue `WM_IME_*` messages.

use std::cell::RefCell;
use std::collections::VecDeque;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Input::Ime::{
    ImmGetContext, ImmReleaseContext, ImmSetCompositionStringW, HIMC, SCS_SETSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, INSERT_TEXT_AT_SELECTION_FLAGS, ITfComposition, ITfCompositionSink,
    ITfCompositionSink_Impl, ITfContext, ITfContextComposition, ITfEditSession,
    ITfEditSession_Impl, ITfInsertAtSelection, ITfThreadMgr, TF_ES_READWRITE, TF_ES_SYNC,
    TF_IAS_QUERYONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION,
};
use windows::core::{implement, Interface, Ref, Result as WinResult};

use crate::{Error, InputMethodEvent, InputMethodState, Result};

// ── COM: direct commit (no active composition) ─────────────────────────────

#[implement(ITfEditSession)]
struct CommitSession {
    text: Vec<u16>,
    context: ITfContext,
}

impl ITfEditSession_Impl for CommitSession_Impl {
    fn DoEditSession(&self, ec: u32) -> WinResult<()> {
        unsafe {
            let ins: ITfInsertAtSelection = self.context.cast()?;
            ins.InsertTextAtSelection(ec, INSERT_TEXT_AT_SELECTION_FLAGS(0), &self.text)?;
            Ok(())
        }
    }
}

// ── COM: commit while a preedit composition is active ─────────────────────

#[implement(ITfEditSession)]
struct CommitCompositionSession {
    text: Vec<u16>,
    composition: ITfComposition,
}

impl ITfEditSession_Impl for CommitCompositionSession_Impl {
    fn DoEditSession(&self, ec: u32) -> WinResult<()> {
        unsafe {
            let range = self.composition.GetRange()?;
            range.SetText(ec, 0, &self.text)?;
            self.composition.EndComposition(ec)
        }
    }
}

// ── COM: start a new preedit composition ──────────────────────────────────
//
// `out` is a raw pointer to a stack-local `Option<ITfComposition>` owned by
// the caller. Safe because `TF_ES_SYNC` guarantees DoEditSession returns
// before RequestEditSession does, keeping the pointee alive throughout.

struct WriteBack(*mut Option<ITfComposition>);
unsafe impl Send for WriteBack {}
unsafe impl Sync for WriteBack {}

#[implement(ITfEditSession)]
struct StartPreeditSession {
    text: Vec<u16>,
    context: ITfContext,
    out: WriteBack,
}

impl ITfEditSession_Impl for StartPreeditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> WinResult<()> {
        unsafe {
            // Query the insertion point without inserting anything.
            let ins: ITfInsertAtSelection = self.context.cast()?;
            let range = ins.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;

            let comp_ctx: ITfContextComposition = self.context.cast()?;
            let sink: ITfCompositionSink = NullCompositionSink.into();
            let new_comp = comp_ctx.StartComposition(ec, &range, &sink)?;
            new_comp.GetRange()?.SetText(ec, 0, &self.text)?;

            *self.out.0 = Some(new_comp);
            Ok(())
        }
    }
}

// ── COM: update text of an existing preedit composition ───────────────────

#[implement(ITfEditSession)]
struct UpdatePreeditSession {
    text: Vec<u16>,
    composition: ITfComposition,
}

impl ITfEditSession_Impl for UpdatePreeditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> WinResult<()> {
        unsafe {
            let range = self.composition.GetRange()?;
            range.SetText(ec, 0, &self.text)
        }
    }
}

// ── COM: minimal composition sink ─────────────────────────────────────────

#[implement(ITfCompositionSink)]
struct NullCompositionSink;

impl ITfCompositionSink_Impl for NullCompositionSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ec_write: u32,
        _p_composition: Ref<'_, ITfComposition>,
    ) -> WinResult<()> {
        Ok(())
    }
}

// ── TSF state ──────────────────────────────────────────────────────────────

struct TsfState {
    thread_mgr: ITfThreadMgr,
    client_id: u32,
}

impl TsfState {
    fn init() -> Option<Self> {
        unsafe {
            // S_FALSE (1) means COM already initialised on this thread — fine.
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr.is_err() {
                return None;
            }
            let tm: ITfThreadMgr =
                CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).ok()?;
            let tid = tm.Activate().ok()?;
            Some(Self { thread_mgr: tm, client_id: tid })
        }
    }

    fn focused_context(&self) -> Option<ITfContext> {
        unsafe {
            let doc = self.thread_mgr.GetFocus().ok()?;
            doc.GetTop().ok()
        }
    }

    fn request_sync_rw(&self, ctx: &ITfContext, session: &ITfEditSession) -> bool {
        unsafe {
            ctx.RequestEditSession(self.client_id, session, TF_ES_SYNC | TF_ES_READWRITE).is_ok()
        }
    }
}

// ── InputMethod ────────────────────────────────────────────────────────────

pub struct InputMethod {
    active: bool,
    serial: u32,
    state: InputMethodState,
    events: VecDeque<InputMethodEvent>,
    composing: bool,
    tsf: Option<TsfState>,
    /// Active TSF preedit composition, if any.
    composition: RefCell<Option<ITfComposition>>,
}

impl InputMethod {
    pub fn new() -> Result<Self> {
        Ok(Self {
            active: true,
            serial: 0,
            state: InputMethodState::new(),
            events: VecDeque::new(),
            composing: false,
            tsf: TsfState::init(),
            composition: RefCell::new(None),
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

    pub fn commit_string(&self, text: &str) -> Result<()> {
        let wide: Vec<u16> = text.encode_utf16().collect();

        // Path A: an active composition is in progress — update its text and end it.
        if let Some(comp) = self.composition.borrow_mut().take() {
            if let Some(tsf) = &self.tsf {
                if let Some(ctx) = tsf.focused_context() {
                    let session: ITfEditSession = CommitCompositionSession {
                        text: wide.clone(),
                        composition: comp,
                    }
                    .into();
                    if tsf.request_sync_rw(&ctx, &session) {
                        return Ok(());
                    }
                }
            }
        }

        // Path B: no composition — insert directly at the current selection.
        if let Some(tsf) = &self.tsf {
            if let Some(ctx) = tsf.focused_context() {
                let session: ITfEditSession =
                    CommitSession { text: wide.clone(), context: ctx.clone() }.into();
                if tsf.request_sync_rw(&ctx, &session) {
                    return Ok(());
                }
            }
        }

        // Fallback: SendInput Unicode simulation.
        self.send_input_string(&wide)
    }

    pub fn set_preedit_string(
        &self,
        text: &str,
        _cursor_begin: i32,
        _cursor_end: i32,
    ) -> Result<()> {
        let wide: Vec<u16> = text.encode_utf16().collect();

        // Update existing composition range; if that fails, use IMM32 directly.
        if let Some(comp) = self.composition.borrow().as_ref().map(|c| c.clone()) {
            if let Some(tsf) = &self.tsf {
                if let Some(ctx) = tsf.focused_context() {
                    let session: ITfEditSession =
                        UpdatePreeditSession { text: wide.clone(), composition: comp }.into();
                    if tsf.request_sync_rw(&ctx, &session) {
                        return Ok(());
                    }
                }
            }
            return self.imm32_set_preedit(&wide);
        }

        // Start a new composition.
        if let Some(tsf) = &self.tsf {
            if let Some(ctx) = tsf.focused_context() {
                let mut new_comp: Option<ITfComposition> = None;
                let session: ITfEditSession = StartPreeditSession {
                    text: wide.clone(),
                    context: ctx.clone(),
                    out: WriteBack(&mut new_comp as *mut _),
                }
                .into();
                if tsf.request_sync_rw(&ctx, &session) {
                    *self.composition.borrow_mut() = new_comp;
                    return Ok(());
                }
            }
        }

        // Fallback: IMM32.
        self.imm32_set_preedit(&wide)
    }

    fn imm32_set_preedit(&self, wide: &[u16]) -> Result<()> {
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

    fn send_input_string(&self, wide: &[u16]) -> Result<()> {
        for &code in wide {
            self.send_unicode_char(code)?;
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
