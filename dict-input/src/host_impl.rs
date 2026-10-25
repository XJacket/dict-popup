//! Win32 implementation of the host layer.
//!
//! This module (and its private children) is the only place in the crate that
//! talks to Win32. The public surface lives in [`crate::host`].
//!
//! Threading model (docs/DESIGN.md §3):
//!
//! * the **host thread** owns a hidden top-level window, the global hotkey, the
//!   tray icon and the command queue; it runs one `GetMessageW` loop and is the
//!   only thread that calls the [`EventSink`];
//! * the **capture threads** ([`uia`]) do UI Automation and post their results
//!   back to the host thread as private window messages;
//! * the **UI thread** only ever calls [`Host::send`](crate::Host::send), which
//!   sends over an mpsc channel and posts a wake-up message, so it never blocks
//!   on Win32.
//!
//! Initialisation order: [`start`] sets the process DPI awareness
//! (`PER_MONITOR_AWARE_V2`) *before* any window exists, then checks the
//! single-instance mutex, then creates the host window. The UI must call
//! `Host::start` before creating its own window for DPI awareness to apply; if
//! it cannot, `start` still succeeds and the DPI call becomes a no-op.

mod autostart_impl;
mod clipboard;
mod pure;
mod tray_impl;
mod uia;
mod win32_util;

/// Diagnostics for the capture path, exposed through `dict_input::debug_capture_report`.
pub(crate) fn debug_capture_report(hwnd: isize) -> String {
    uia::debug_capture_report(hwnd)
}

pub(crate) fn debug_clipboard_read() -> Option<String> {
    clipboard::read_text()
}

pub(crate) fn debug_clipboard_write(text: &str) {
    clipboard::write_text(text);
}

pub(crate) fn debug_capture_once(target_hwnd: Option<isize>) -> String {
    uia::debug_capture_once(target_hwnd)
}

pub(crate) fn debug_type_text(text: &str) -> usize {
    clipboard::debug_type_text(text)
}

pub(crate) fn debug_press_combo(keys: &[u16]) {
    clipboard::debug_press_combo(keys)
}

/// Run the clipboard capture once and return the text or the error message.
pub(crate) fn debug_capture_selection() -> Result<String, String> {
    clipboard::capture()
        .map(|selection| selection.text)
        .map_err(|error| error.to_string())
}

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE,
    HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW,
    GetForegroundWindow, GetMessageW, GetWindowLongPtrW, IsWindow, KillTimer, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetWindowLongPtrW, TranslateMessage,
    CREATESTRUCTW, GWLP_USERDATA, HCURSOR, HICON, HMENU, WNDCLASSW, WINDOW_EX_STYLE, WM_DESTROY,
    WM_ENDSESSION, WM_HOTKEY, WM_NCCREATE, WM_QUERYENDSESSION, WM_TIMER, WNDCLASS_STYLES,
    WS_OVERLAPPED,
};

use crate::host::{Command, Event, EventSink, Host, HostError, HostOptions};
use crate::hotkey::Hotkey;
use crate::selection::SelectionError;

/// Handle used by the UI thread to wake the host message loop.
///
/// The window handle is kept as an `isize` so [`Host`] stays `Send + Sync`
/// without wrapping a raw pointer; posting to a destroyed window just fails and
/// is ignored.
#[derive(Clone, Copy)]
pub(crate) struct Waker {
    hwnd: isize,
}

impl Waker {
    fn new(hwnd: HWND) -> Self {
        Self {
            hwnd: hwnd.0 as isize,
        }
    }

    pub(crate) fn wake(&self) {
        if self.hwnd == 0 {
            return;
        }
        // SAFETY: `hwnd` is the host window created on the host thread.
        // PostMessageW is documented as thread-safe and only fails after the
        // window is destroyed, which is harmless here.
        let _ = unsafe {
            PostMessageW(
                HWND(self.hwnd as *mut c_void),
                pure::WAKE_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }
}

/// Wins the race to become "the" instance; the handle is intentionally kept for
/// the whole process lifetime and never closed.
static SINGLE_INSTANCE_MUTEX: AtomicIsize = AtomicIsize::new(0);

/// Queue of events produced by the capture threads. The host thread drains it
/// and is therefore the only caller of the [`EventSink`].
struct Notifier {
    hwnd: AtomicIsize,
    queue: Mutex<VecDeque<Event>>,
}

impl Notifier {
    fn new() -> Self {
        Self {
            hwnd: AtomicIsize::new(0),
            queue: Mutex::new(VecDeque::new()),
        }
    }

    fn set_hwnd(&self, hwnd: HWND) {
        self.hwnd.store(hwnd.0 as isize, Ordering::Release);
    }

    fn push(&self, event: Event) {
        match self.queue.lock() {
            Ok(mut queue) => queue.push_back(event),
            Err(poisoned) => poisoned.into_inner().push_back(event),
        }

        let raw = self.hwnd.load(Ordering::Acquire);
        if raw == 0 {
            return;
        }
        // SAFETY: `raw` is the host window handle; posting is thread-safe.
        let _ = unsafe {
            PostMessageW(
                HWND(raw as *mut c_void),
                pure::CAPTURE_READY_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }

    fn drain(&self) -> Vec<Event> {
        match self.queue.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(poisoned) => poisoned.into_inner().drain(..).collect(),
        }
    }
}

/// All state owned by the host thread. Lives in a `Box` whose raw pointer is
/// stored in the window's `GWLP_USERDATA`; only the host thread ever touches it.
struct HostState {
    hwnd: HWND,
    sink: EventSink,
    commands: Receiver<Command>,
    notifier: Arc<Notifier>,
    tray: tray_impl::Tray,
    capture: Option<uia::Capture>,
    hotkey_id: Option<i32>,
    hotkey: Option<Hotkey>,
    show_settings_message: u32,
    taskbar_created_message: u32,
    /// Test hook: `DICT_POPUP_CAPTURE_HWND=<hwnd>` forces the capture target
    /// instead of the foreground window, so the UIA path can be tested without
    /// being able to steal foreground focus from an automated session.
    capture_override: Option<isize>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub(crate) fn start(options: HostOptions, sink: EventSink) -> Result<Host, HostError> {
    // Must run before *any* window exists in this process (docs/DESIGN.md §5).
    init_dpi_awareness();

    acquire_single_instance()?;

    let (command_tx, command_rx) = crate::host::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<Waker, HostError>>();

    let thread = std::thread::Builder::new()
        .name("dict-input-host".to_string())
        .spawn(move || host_thread(options, sink, command_rx, ready_tx))
        .map_err(|error| HostError::Win32(format!("无法创建 Host 线程：{error}")))?;

    // The handshake is the only place the UI thread waits, and only for the
    // few milliseconds it takes to create the window. All later Win32 work
    // happens on the host thread.
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(waker)) => Ok(Host::from_parts(command_tx, thread, waker)),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => Err(HostError::Win32(
            "Host 线程启动超时（5 秒内未完成窗口创建）".to_string(),
        )),
    }
}

/// Set `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` once per process.
///
/// A failure here is expected when the process is already DPI-aware (manifest,
/// or the UI created a window first); it is not fatal.
pub(crate) fn init_dpi_awareness() {
    // SAFETY: process-wide setting with no pointer arguments; safe to call from
    // any thread before the first window exists.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Create the named mutex; if another instance owns it, poke that instance and
/// report [`HostError::AlreadyRunning`].
fn acquire_single_instance() -> Result<(), HostError> {
    let name = win32_util::wide(pure::MUTEX_NAME);
    // SAFETY: `name` is NUL-terminated and outlives the call; a null
    // SECURITY_ATTRIBUTES means the default security descriptor.
    let handle = unsafe { CreateMutexW(None, BOOL::from(true), PCWSTR::from_raw(name.as_ptr())) }
        .map_err(|error| HostError::Win32(format!("CreateMutexW 失败：{}", error.message())))?;

    // SAFETY: GetLastError is only meaningful immediately after CreateMutexW.
    // A valid handle plus ERROR_ALREADY_EXISTS means another instance won.
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if !already_running {
        // Keep the handle for the process lifetime: dropping it would let a
        // second instance start while this one is still alive.
        SINGLE_INSTANCE_MUTEX.store(handle.0 as isize, Ordering::SeqCst);
        return Ok(());
    }

    // SAFETY: this is our extra handle to the mutex; we never use it.
    let _ = unsafe { CloseHandle(handle) };

    // If this process already owns the mutex, the ERROR_ALREADY_EXISTS came
    // from our own handle: the UI is restarting the Host in-process, which is
    // allowed (there is still only one tray icon because the old window is
    // gone).
    if SINGLE_INSTANCE_MUTEX.load(Ordering::SeqCst) != 0 {
        return Ok(());
    }

    notify_running_instance();
    Err(HostError::AlreadyRunning)
}

/// Ask the first instance to surface its settings window.
///
/// The first instance may still be starting up, so poll briefly for its window.
/// This runs on the second instance's UI thread before the UI exists; a few
/// hundred milliseconds is acceptable and bounded.
fn notify_running_instance() {
    let class_name = win32_util::wide(pure::WINDOW_CLASS);
    let class_name = PCWSTR::from_raw(class_name.as_ptr());
    let message_name = win32_util::wide(pure::SHOW_SETTINGS_MESSAGE);

    let mut target = HWND::default();
    for attempt in 0..20 {
        // SAFETY: the class name is NUL-terminated; an empty window name asks
        // for any window of that class.
        match unsafe { FindWindowW(class_name, PCWSTR::null()) } {
            Ok(hwnd) if !hwnd.is_invalid() => {
                target = hwnd;
                break;
            }
            _ => {
                if attempt < 19 {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
    if target.is_invalid() {
        return;
    }

    // SAFETY: NUL-terminated message name; registered in the first instance too.
    let message = unsafe { RegisterWindowMessageW(PCWSTR::from_raw(message_name.as_ptr())) };
    if message == 0 {
        return;
    }
    // SAFETY: `target` is a live window handle; posting does not require it to
    // belong to this process.
    let _ = unsafe { PostMessageW(target, message, WPARAM(0), LPARAM(0)) };
}

// ---------------------------------------------------------------------------
// Host thread
// ---------------------------------------------------------------------------

fn host_thread(
    options: HostOptions,
    sink: EventSink,
    commands: Receiver<Command>,
    ready: Sender<Result<Waker, HostError>>,
) {
    // SAFETY: a null module name returns this executable's module handle.
    let module = match unsafe { GetModuleHandleW(PCWSTR::null()) } {
        Ok(module) => module,
        Err(error) => {
            let _ = ready.send(Err(HostError::Win32(format!(
                "GetModuleHandleW 失败：{}",
                error.message()
            ))));
            return;
        }
    };
    // HMODULE and HINSTANCE are the same handle, just distinct newtypes.
    let instance = HINSTANCE(module.0);

    if let Err(error) = ensure_window_class(instance) {
        let _ = ready.send(Err(error));
        return;
    }

    // Register the session-scoped message ids *before* the window exists, so a
    // second instance can never find the window and post a message id that this
    // process does not know about yet.
    let show_settings_message = register_message(pure::SHOW_SETTINGS_MESSAGE);
    let taskbar_created_message = register_message(pure::TASKBAR_CREATED_MESSAGE);

    let notifier = Arc::new(Notifier::new());
    let state = Box::new(HostState {
        hwnd: HWND::default(),
        sink,
        commands,
        notifier: Arc::clone(&notifier),
        tray: tray_impl::Tray::new(),
        capture: None,
        hotkey_id: None,
        hotkey: None,
        show_settings_message,
        taskbar_created_message,
        capture_override: std::env::var("DICT_POPUP_CAPTURE_HWND")
            .ok()
            .and_then(|value| value.trim().parse::<isize>().ok()),
    });
    let state_ptr = Box::into_raw(state);

    // SAFETY: `state_ptr` was produced by `Box::into_raw` immediately above and
    // is not freed on this path; `create_window` hands it to the window
    // procedure, which reclaims it exactly once in WM_NCDESTROY.
    let hwnd = match unsafe { create_window(instance, state_ptr) } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            // SAFETY: WM_NCDESTROY never frees the box, and the window never
            // became visible, so we still own it here.
            drop(unsafe { Box::from_raw(state_ptr) });
            let _ = ready.send(Err(error));
            return;
        }
    };

    // SAFETY: `state_ptr` is live, owned by this thread, and not shared yet.
    let startup_events = unsafe {
        (*state_ptr).hwnd = hwnd;
        notifier.set_hwnd(hwnd);
        init_host(state_ptr, options)
    };

    // From here on `Host` exists on the UI thread and may send commands.
    let _ = ready.send(Ok(Waker::new(hwnd)));

    // SAFETY: `state_ptr` is the live box owned by this thread; the sink is
    // only ever called from here until the loop exits.
    unsafe {
        for event in startup_events {
            ((*state_ptr).sink)(event);
        }
        message_loop(state_ptr);
        // SAFETY: the window has been destroyed and the loop exited; nobody
        // else holds this pointer. Reclaim the allocation exactly once.
        drop(Box::from_raw(state_ptr));
    }
}

unsafe fn init_host(state: *mut HostState, options: HostOptions) -> Vec<Event> {
    let mut events = Vec::new();
    let hwnd = (*state).hwnd;

    (*state).tray.init(hwnd, &options.tooltip);

    match options.hotkey {
        // SAFETY: hwnd belongs to this thread; the hotkey is validated by the
        // parser before it reaches us.
        Ok(hotkey) => match unsafe { register_hotkey(hwnd, pure::HOTKEY_ID_PRIMARY, &hotkey) } {
            Ok(()) => {
                (*state).hotkey_id = Some(pure::HOTKEY_ID_PRIMARY);
                (*state).hotkey = Some(hotkey);
            }
            Err(reason) => events.push(Event::HotkeyFailed(pure::hotkey_failure_message(
                &hotkey,
                None,
                &reason,
            ))),
        },
        Err(error) => events.push(Event::HotkeyFailed(format!("热键不可用：{error}"))),
    }

    let notifier = Arc::clone(&(*state).notifier);
    (*state).capture = Some(uia::Capture::start(move |event| notifier.push(event)));

    events
}

unsafe fn message_loop(state: *mut HostState) {
    let mut message = MSG::default();

    loop {
        // Deliver capture results and commands even if the wake-up post was
        // coalesced or lost.
        flush_capture_events(state);
        drain_commands(state);

        // SAFETY: `message` is a valid MSG storage owned by this frame.
        let result = unsafe { GetMessageW(&mut message, HWND::default(), 0, 0) };
        if result.0 == 0 || result.0 == -1 {
            break;
        }

        flush_capture_events(state);
        drain_commands(state);

        // SAFETY: `message` was filled in by GetMessageW.
        let _ = unsafe { TranslateMessage(&message) };
        unsafe { DispatchMessageW(&message) };
    }

    // If the loop ended because GetMessageW failed (not because WM_QUIT was
    // posted by WM_DESTROY), make sure the window and with it the tray icon go
    // away.
    let hwnd = (*state).hwnd;
    // SAFETY: hwnd may or may not still exist; IsWindow is the check.
    if unsafe { IsWindow(hwnd) }.as_bool() {
        // SAFETY: the window belongs to this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
    }
}

unsafe fn flush_capture_events(state: *mut HostState) {
    let events = (*state).notifier.drain();
    if events.is_empty() {
        return;
    }
    let sink = &(*state).sink;
    for event in events {
        sink(event);
    }
}

unsafe fn drain_commands(state: *mut HostState) {
    loop {
        let command = match (*state).commands.try_recv() {
            Ok(command) => command,
            Err(_) => return,
        };

        match command {
            Command::SetHotkey(hotkey) => command_set_hotkey(state, hotkey),
            Command::SetTooltip(tooltip) => (*state).tray.set_tip(&tooltip),
            Command::Notify { title, body } => {
                let _ = (*state).tray.notify(&title, &body);
            }
            Command::Shutdown => {
                let hwnd = (*state).hwnd;
                // SAFETY: no reference to `*state` is alive here. WM_DESTROY
                // may re-enter wnd_proc, which re-derives short-lived borrows.
                if unsafe { DestroyWindow(hwnd) }.is_err() {
                    // The window is already gone, so WM_DESTROY will not post
                    // WM_QUIT; do it ourselves or the loop would never end.
                    PostQuitMessage(0);
                }
                return;
            }
        }
    }
}

/// Register the replacement hotkey under the *other* id first and only release
/// the old one once the new registration succeeded (docs/DESIGN.md §9.9).
unsafe fn command_set_hotkey(state: *mut HostState, new_hotkey: Option<Hotkey>) {
    let hwnd = (*state).hwnd;

    match new_hotkey {
        None => {
            if let Some(id) = (*state).hotkey_id.take() {
                // SAFETY: id was registered for this window.
                let _ = unsafe { UnregisterHotKey(hwnd, id) };
            }
            (*state).hotkey = None;
        }
        Some(hotkey) => {
            if (*state).hotkey == Some(hotkey) && (*state).hotkey_id.is_some() {
                return;
            }

            let active_id = (*state).hotkey_id;
            let candidate_id = pure::next_hotkey_id(active_id);
            // SAFETY: hwnd belongs to this thread; this is the new registration
            // that must succeed before the old one is released.
            match unsafe { register_hotkey(hwnd, candidate_id, &hotkey) } {
                Ok(()) => {
                    if let Some(old_id) = active_id {
                        // SAFETY: the old id belongs to this window and the new
                        // one is already live, so no gap exists.
                        let _ = unsafe { UnregisterHotKey(hwnd, old_id) };
                    }
                    (*state).hotkey_id = Some(candidate_id);
                    (*state).hotkey = Some(hotkey);
                }
                Err(reason) => {
                    // Keep the old registration untouched.
                    let previous = (*state).hotkey;
                    ((*state).sink)(Event::HotkeyFailed(pure::hotkey_failure_message(
                        &hotkey,
                        previous.as_ref(),
                        &reason,
                    )));
                }
            }
        }
    }
}

unsafe fn register_hotkey(hwnd: HWND, id: i32, hotkey: &Hotkey) -> Result<(), String> {
    // SAFETY: `hwnd` belongs to this thread. `Modifiers::to_win32` already
    // includes MOD_NOREPEAT.
    unsafe {
        RegisterHotKey(
            hwnd,
            id,
            HOT_KEY_MODIFIERS(hotkey.mods.to_win32()),
            hotkey.vk.0,
        )
    }
    .map_err(|error| error.message())
}

// ---------------------------------------------------------------------------
// Window class / window / window proc
// ---------------------------------------------------------------------------

fn register_message(name: &str) -> u32 {
    let wide_name = win32_util::wide(name);
    // SAFETY: the string is NUL-terminated and outlives the call.
    unsafe { RegisterWindowMessageW(PCWSTR::from_raw(wide_name.as_ptr())) }
}

fn ensure_window_class(instance: HINSTANCE) -> Result<(), HostError> {
    let class_name = win32_util::wide(pure::WINDOW_CLASS);
    let window_class = WNDCLASSW {
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON(std::ptr::null_mut()),
        hCursor: HCURSOR(std::ptr::null_mut()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
    };

    // SAFETY: `window_class` is fully initialised and the strings it points at
    // outlive the call.
    let atom = unsafe { RegisterClassW(&window_class) };
    if atom != 0 {
        return Ok(());
    }

    // SAFETY: read immediately after the failing call.
    let code = unsafe { GetLastError() };
    if code == ERROR_CLASS_ALREADY_EXISTS {
        // Another Host in the same process already registered it.
        return Ok(());
    }
    Err(HostError::Win32(format!(
        "RegisterClassW 失败：{}（错误码 {}）",
        win32_util::error_text(code.0),
        code.0
    )))
}

unsafe fn create_window(instance: HINSTANCE, state: *mut HostState) -> Result<HWND, HostError> {
    let class_name = win32_util::wide(pure::WINDOW_CLASS);
    let title = win32_util::wide(pure::WINDOW_TITLE);

    // SAFETY: style 0 with a null parent creates a hidden top-level window (not
    // HWND_MESSAGE, which would not receive TaskbarCreated broadcasts). The
    // string buffers outlive the call; `state` is handed to WM_NCCREATE.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            instance,
            Some(state as *const c_void),
        )
    }
    .map_err(|error| HostError::Win32(format!("CreateWindowExW 失败：{}", error.message())))
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE Windows passes a CREATESTRUCTW that is valid
        // for the duration of the call, and lpCreateParams is the pointer we
        // handed to CreateWindowExW.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        if !create.lpCreateParams.is_null() {
            // SAFETY: storing our live Box<HostState> pointer; it is never
            // freed by the window proc (see host_thread).
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        }
        // SAFETY: `create` is a valid CREATESTRUCTW for this call.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    // SAFETY: GWLP_USERDATA is only ever set to the live Box<HostState> pointer
    // in WM_NCCREATE, on this same thread.
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut HostState;
    if state.is_null() {
        // SAFETY: default processing for a window that has no host state yet.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    // SAFETY: `state` is live for the whole message loop; each handler below
    // re-derives its own short-lived borrows so that TrackPopupMenu's nested
    // message loop cannot alias an outstanding `&mut`.
    unsafe {
        let show_settings = (*state).show_settings_message;
        if show_settings != 0 && message == show_settings {
            ((*state).sink)(Event::SecondLaunch);
            return LRESULT(0);
        }
        let taskbar_created = (*state).taskbar_created_message;
        if taskbar_created != 0 && message == taskbar_created {
            (*state).tray.re_add();
            return LRESULT(0);
        }
    }

    match message {
        WM_HOTKEY => {
            // SAFETY: as above.
            unsafe { on_hotkey(state, wparam) };
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == pure::TRAY_RETRY_TIMER {
                // SAFETY: as above.
                unsafe { (*state).tray.on_retry_timer() };
            }
            LRESULT(0)
        }
        m if m == pure::TRAY_CALLBACK_MESSAGE => {
            // SAFETY: as above.
            unsafe { on_tray_callback(state, lparam) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: as above.
            unsafe { on_destroy(state, hwnd) };
            LRESULT(0)
        }
        WM_QUERYENDSESSION => {
            // SAFETY: as above.
            unsafe { (*state).tray.remove() };
            LRESULT(1)
        }
        WM_ENDSESSION => {
            // SAFETY: as above. wParam == 0 means the shutdown was cancelled,
            // so put the icon back.
            unsafe {
                if wparam.0 != 0 {
                    (*state).tray.remove();
                } else {
                    (*state).tray.re_add();
                }
            }
            LRESULT(0)
        }
        // SAFETY: unhandled messages get default processing.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe fn on_hotkey(state: *mut HostState, wparam: WPARAM) {
    let id = wparam.0 as i32;
    if (*state).hotkey_id != Some(id) {
        return;
    }

    ((*state).sink)(Event::Hotkey);

    // Remember *which window* the user was working in. UIA's "focused element"
    // is unreliable across toolkits (WinUI/XAML apps often report the top-level
    // window, which exposes no text pattern), so the target window is captured
    // here and searched for a text element by the worker.
    let target = match (*state).capture_override {
        Some(hwnd) => HWND(hwnd as *mut core::ffi::c_void),
        // SAFETY: no arguments; returns the foreground window or null.
        None => unsafe { GetForegroundWindow() },
    };

    let request = match &mut (*state).capture {
        Some(capture) => capture.request(target),
        None => Err(SelectionError::Automation("取词服务未启动".to_string())),
    };
    if let Err(error) = request {
        ((*state).sink)(Event::SelectionFailed(error));
    }
}

unsafe fn on_tray_callback(state: *mut HostState, lparam: LPARAM) {
    let icon_id = pure::tray_event_icon_id(lparam.0);
    if icon_id != 0 && icon_id != pure::TRAY_ICON_ID {
        return;
    }
    if !pure::tray_opens_menu(pure::tray_event_message(lparam.0)) {
        return;
    }

    let hwnd = (*state).hwnd;
    // No borrow of `*state` across show_menu: TrackPopupMenu pumps messages and
    // can re-enter this window proc.
    let action = tray_impl::show_menu(hwnd);
    if let Some(action) = action {
        ((*state).sink)(Event::Tray(action));
    }
}

unsafe fn on_destroy(state: *mut HostState, hwnd: HWND) {
    (*state).tray.remove();
    if let Some(id) = (*state).hotkey_id.take() {
        // SAFETY: id was registered for this window.
        let _ = unsafe { UnregisterHotKey(hwnd, id) };
    }
    // SAFETY: the timer id is ours; killing a non-existent timer is a no-op.
    let _ = unsafe { KillTimer(hwnd, pure::TRAY_RETRY_TIMER) };
    PostQuitMessage(0);
}

// ---------------------------------------------------------------------------
// Autostart bridge (the public API lives in `crate::autostart`)
// ---------------------------------------------------------------------------

pub(crate) fn autostart_query() -> std::io::Result<bool> {
    autostart_impl::query()
}

pub(crate) fn autostart_set(enabled: bool) -> std::io::Result<()> {
    autostart_impl::set(enabled)
}
