//! Tray icon (`Shell_NotifyIconW`) and its context menu.
//!
//! The icon is created on the host thread's hidden window. `NIM_SETVERSION`
//! switches to `NOTIFYICON_VERSION_4`, so the callback's `lParam` carries the
//! event message in its low word. If `NIM_ADD` fails (Explorer not ready yet) a
//! timer retries instead of pretending the icon exists; `TaskbarCreated`
//! re-adds it after an Explorer restart.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, POINT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    NOTIFY_ICON_DATA_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, IDI_APPLICATION, KillTimer, LoadIconW,
    PostMessageW, SetForegroundWindow, SetTimer, TrackPopupMenu, HICON, MF_SEPARATOR, MF_STRING,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use crate::tray::TrayAction;

use super::pure;
use super::win32_util::wide;

const MENU_OPEN_SETTINGS: &str = "打开设置";
const MENU_RELOAD_DICTIONARIES: &str = "重新加载词典";
const MENU_QUIT: &str = "退出";

pub(crate) struct Tray {
    hwnd: HWND,
    icon: Option<HICON>,
    tip: Vec<u16>,
    added: bool,
    retries_left: u32,
}

impl Tray {
    pub(crate) fn new() -> Self {
        Self {
            hwnd: HWND::default(),
            icon: None,
            tip: Vec::new(),
            added: false,
            retries_left: 0,
        }
    }

    /// Attach the icon to `hwnd` and try to add it once.
    ///
    /// The icon is currently the stock `IDI_APPLICATION`; the app icon resource
    /// (`assets/dict-popup.ico`) is owned by the release-engineering agent and
    /// is not loaded yet. See the module report.
    pub(crate) fn init(&mut self, hwnd: HWND, tooltip: &str) {
        self.hwnd = hwnd;
        self.tip = pure::truncate_utf16(tooltip, pure::SZ_TIP_UNITS);
        // SAFETY: with a NULL module handle IDI_APPLICATION selects a
        // predefined system icon; the call takes no other inputs.
        self.icon = unsafe { LoadIconW(HINSTANCE::default(), IDI_APPLICATION) }.ok();
        self.retries_left = pure::TRAY_MAX_RETRIES;
        self.attempt();
    }

    /// Build a `NOTIFYICONDATAW` with the fields that are always valid.
    fn data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW::default();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = pure::TRAY_ICON_ID;
        data.uFlags = flags;
        data.uCallbackMessage = pure::TRAY_CALLBACK_MESSAGE;
        if let Some(icon) = self.icon {
            data.hIcon = icon;
        }
        pure::write_sz(&mut data.szTip, &self.tip);
        data
    }

    fn add(&mut self) -> bool {
        let mut flags = NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        if self.icon.is_some() {
            flags |= NIF_ICON;
        }
        let data = self.data(flags);
        // SAFETY: `data` is a fully initialised NOTIFYICONDATAW whose cbSize
        // matches the struct, and it outlives the call.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            return false;
        }

        // Version 4 changes the callback payload (see tray_event_message).
        let mut version = self.data(NOTIFY_ICON_DATA_FLAGS(0));
        // SAFETY (union write): the uVersion arm is the correct one for
        // NIM_SETVERSION; writing a union field is not an unsafe operation, but
        // the semantic precondition is documented here.
        version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: same struct validity argument as above.
        let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &version) };

        self.added = true;
        true
    }

    /// One add attempt; on failure arm the retry timer.
    fn attempt(&mut self) {
        if self.add() {
            // SAFETY: the timer id is the one this window created for us.
            let _ = unsafe { KillTimer(self.hwnd, pure::TRAY_RETRY_TIMER) };
            self.retries_left = pure::TRAY_MAX_RETRIES;
            return;
        }

        if self.retries_left > 0 {
            self.retries_left -= 1;
            // SAFETY: no timer callback is used, so any thread may call SetTimer
            // with a window handle; the message is posted to our own window.
            unsafe {
                SetTimer(
                    self.hwnd,
                    pure::TRAY_RETRY_TIMER,
                    pure::TRAY_RETRY_INTERVAL_MS,
                    None,
                )
            };
        } else {
            // SAFETY: the timer id is ours.
            let _ = unsafe { KillTimer(self.hwnd, pure::TRAY_RETRY_TIMER) };
        }
    }

    /// `WM_TIMER` for the retry timer.
    pub(crate) fn on_retry_timer(&mut self) {
        if self.added {
            // SAFETY: the timer id is ours.
            let _ = unsafe { KillTimer(self.hwnd, pure::TRAY_RETRY_TIMER) };
            return;
        }
        self.attempt();
    }

    /// Explorer restarted (`TaskbarCreated`) or a shutdown was cancelled: the
    /// icon is gone even though `added` may still be true, so clear it and add
    /// again.
    pub(crate) fn re_add(&mut self) {
        self.added = false;
        self.retries_left = pure::TRAY_MAX_RETRIES;
        self.attempt();
    }

    pub(crate) fn remove(&mut self) {
        // Stop any pending retry first: after a session-end request we must not
        // put the icon back.
        // SAFETY: the timer id is ours; killing a missing timer is harmless.
        let _ = unsafe { KillTimer(self.hwnd, pure::TRAY_RETRY_TIMER) };
        if !self.added {
            return;
        }
        let data = self.data(NOTIFY_ICON_DATA_FLAGS(0));
        // SAFETY: `data` is fully initialised for this icon id; NIM_DELETE is
        // safe to call even if the shell already dropped the icon.
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        self.added = false;
    }

    pub(crate) fn set_tip(&mut self, tooltip: &str) {
        let units = pure::truncate_utf16(tooltip, pure::SZ_TIP_UNITS);
        if units == self.tip {
            return;
        }
        self.tip = units;
        if !self.added {
            return;
        }
        let data = self.data(NIF_TIP | NIF_SHOWTIP);
        // SAFETY: same validity argument as in `add`.
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    /// Show a balloon. Returns false when there is no icon to attach it to.
    pub(crate) fn notify(&mut self, title: &str, body: &str) -> bool {
        if !self.added {
            return false;
        }
        let mut data = self.data(NIF_INFO);
        pure::write_sz(
            &mut data.szInfo,
            &pure::truncate_utf16(body, pure::SZ_INFO_UNITS),
        );
        pure::write_sz(
            &mut data.szInfoTitle,
            &pure::truncate_utf16(title, pure::SZ_INFO_TITLE_UNITS),
        );
        data.dwInfoFlags = NIIF_INFO;
        // SAFETY (union write): the uTimeout arm is the one NIF_INFO uses.
        data.Anonymous.uTimeout = 10_000;
        // SAFETY: `data` is fully initialised for this icon id.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Pop up the tray menu and map the chosen item to a [`TrayAction`].
///
/// Free function on purpose: `TrackPopupMenu` runs a nested message loop, so no
/// borrow of the host state may be alive while it runs (see `host_impl`'s
/// re-entrancy note).
pub(crate) fn show_menu(hwnd: HWND) -> Option<TrayAction> {
    // SAFETY: CreatePopupMenu returns an owned menu handle.
    let menu = unsafe { CreatePopupMenu() }.ok()?;

    let mut append_ok = true;
    for (text, action) in [
        (MENU_OPEN_SETTINGS, TrayAction::OpenSettings),
        (MENU_RELOAD_DICTIONARIES, TrayAction::ReloadDictionaries),
    ] {
        let wide_text = wide(text);
        // SAFETY: menu is live; the wide string outlives the call.
        append_ok &= unsafe {
            AppendMenuW(
                menu,
                MF_STRING,
                pure::action_to_menu_id(action),
                PCWSTR::from_raw(wide_text.as_ptr()),
            )
        }
        .is_ok();
    }
    // SAFETY: menu is live; a separator carries no string.
    append_ok &= unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }.is_ok();
    let quit_text = wide(MENU_QUIT);
    // SAFETY: menu is live; the wide string outlives the call.
    append_ok &= unsafe {
        AppendMenuW(
            menu,
            MF_STRING,
            pure::action_to_menu_id(TrayAction::Quit),
            PCWSTR::from_raw(quit_text.as_ptr()),
        )
    }
    .is_ok();

    if !append_ok {
        // SAFETY: `menu` is the handle we created and no longer need.
        let _ = unsafe { DestroyMenu(menu) };
        return None;
    }

    let mut point = POINT::default();
    // SAFETY: `point` is a valid out-parameter.
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        // SAFETY: as above.
        let _ = unsafe { DestroyMenu(menu) };
        return None;
    }

    // Required so the menu dismisses when the user clicks elsewhere.
    // SAFETY: hwnd is our live hidden window.
    let _ = unsafe { SetForegroundWindow(hwnd) };

    // SAFETY: menu is live, the coordinates come from GetCursorPos and the
    // window handle belongs to this thread. TPM_NONOTIFY + TPM_RETURNCMD means
    // no WM_COMMAND is posted and the selected id is the return value.
    let chosen = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            point.x,
            point.y,
            0,
            hwnd,
            None,
        )
    };

    // SAFETY: `menu` is ours and no longer used after this point.
    let _ = unsafe { DestroyMenu(menu) };
    // SAFETY: flushes the menu's message queue cleanup, per TrackPopupMenu docs.
    let _ = unsafe {
        PostMessageW(
            hwnd,
            WM_NULL,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        )
    };

    pure::menu_id_to_action(chosen.0 as usize)
}
