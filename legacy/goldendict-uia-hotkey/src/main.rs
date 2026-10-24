#![windows_subsystem = "windows"]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    IDI_APPLICATION, MB_ICONERROR, MB_ICONWARNING, MB_OK, MF_STRING, MSG, TPM_NONOTIFY,
    TPM_RETURNCMD, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
    WM_HOTKEY, WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

const HOTKEY_ID: i32 = 1;
const WM_TRAY: u32 = WM_APP + 1;
const ID_OPEN_GOLDENDICT: usize = 1001;
const ID_OPEN_CONFIG: usize = 1002;
const ID_RELOAD: usize = 1003;
const ID_EXIT: usize = 1004;
const TRAY_UID: u32 = 1;

static CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Config {
    hotkey: String,
    goldendict_path: String,
    trim: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Alt+D".to_string(),
            goldendict_path: r"D:\program\GoldenDict-ng\goldendict.exe".to_string(),
            trim: true,
        }
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("GoldenDictUiaHotkey").join("config.json")
}

fn load_config() -> Config {
    let path = config_path();
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<Config>(&text) {
            return cfg;
        }
    }
    let cfg = Config::default();
    save_config(&cfg);
    cfg
}

fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = fs::write(&path, text);
    }
}

fn log_line(message: &str) {
    let path = config_path().with_file_name("app.log");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{ts}] {message}");
    }
}

fn cfg() -> Config {
    CONFIG
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

fn update_cfg(cfg: Config) {
    if let Some(m) = CONFIG.get() {
        *m.lock().unwrap() = cfg;
    }
}

fn vk_for_key(key: &str) -> Option<u32> {
    let up = key.trim().to_ascii_uppercase();
    match up.as_str() {
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "ENTER" | "RETURN" => Some(0x0D),
        "ESC" | "ESCAPE" => Some(0x1B),
        "BACKSPACE" => Some(0x08),
        "DELETE" | "DEL" => Some(0x2E),
        "INSERT" | "INS" => Some(0x2D),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" | "PGUP" => Some(0x21),
        "PAGEDOWN" | "PGDN" => Some(0x22),
        "UP" => Some(0x26),
        "DOWN" => Some(0x28),
        "LEFT" => Some(0x25),
        "RIGHT" => Some(0x27),
        _ => {
            if up.len() == 1 {
                let c = up.chars().next()?;
                if c.is_ascii_alphabetic() {
                    Some(0x41 + (c as u32 - 'A' as u32))
                } else if c.is_ascii_digit() {
                    Some(0x30 + (c as u32 - '0' as u32))
                } else {
                    None
                }
            } else if let Some(n) = up.strip_prefix('F').and_then(|s| s.parse::<u32>().ok()) {
                if (1..=24).contains(&n) {
                    Some(0x70 + n - 1)
                } else {
                    None
                }
            } else {
                None
            }
        }
    }
}

fn parse_hotkey(input: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut mods = HOT_KEY_MODIFIERS(0);
    let mut vk = 0u32;

    for part in input.split('+').map(str::trim).filter(|s| !s.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods = HOT_KEY_MODIFIERS(mods.0 | MOD_CONTROL.0),
            "alt" => mods = HOT_KEY_MODIFIERS(mods.0 | MOD_ALT.0),
            "shift" => mods = HOT_KEY_MODIFIERS(mods.0 | MOD_SHIFT.0),
            "win" | "super" => mods = HOT_KEY_MODIFIERS(mods.0 | MOD_WIN.0),
            _ => vk = vk_for_key(part)?,
        }
    }

    if vk == 0 {
        return None;
    }
    mods = HOT_KEY_MODIFIERS(mods.0 | MOD_NOREPEAT.0);
    Some((mods, vk))
}

unsafe fn register_hotkey(hwnd: HWND, cfg: &Config) -> bool {
    let Some((mods, vk)) = parse_hotkey(&cfg.hotkey) else {
        log_line(&format!("parse hotkey failed: {}", cfg.hotkey));
        return false;
    };
    let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
    let ok = RegisterHotKey(hwnd, HOTKEY_ID, mods, vk).is_ok();
    log_line(&format!("register hotkey {:?} vk={vk} -> {ok}", cfg.hotkey));
    ok
}

unsafe fn selected_text() -> Option<String> {
    let automation: IUIAutomation =
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
    let element = automation.GetFocusedElement().ok()?;
    let pattern: IUIAutomationTextPattern =
        element.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
    let selection = pattern.GetSelection().ok()?;
    let count = selection.Length().ok()?;

    let mut result = String::new();
    for i in 0..count {
        if let Ok(range) = selection.GetElement(i) {
            if let Ok(text) = range.GetText(-1) {
                result.push_str(&text.to_string());
            }
        }
    }
    Some(result)
}

unsafe fn translate_selection() {
    log_line("hotkey fired");
    let cfg = cfg();

    let mut selected = selected_text();
    let empty = selected
        .as_deref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true);
    if empty {
        // First UIA call in Chromium-based apps can enable accessibility lazily; retry once.
        std::thread::sleep(Duration::from_millis(150));
        selected = selected_text();
    }

    match selected {
        Some(text) => {
            log_line(&format!("selected {} chars: {:?}", text.chars().count(), text));
            let text = if cfg.trim { text.trim() } else { text.as_str() };
            if !text.is_empty() {
                let result = Command::new(&cfg.goldendict_path).arg(text).spawn();
                log_line(&format!("spawn goldendict -> {result:?}"));
                return;
            }
        }
        None => log_line("UIA selected_text returned None"),
    }
    let _ = MessageBeep(MB_ICONWARNING);
}

unsafe fn tray_add(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    if let Ok(icon) = LoadIconW(None, IDI_APPLICATION) {
        nid.hIcon = icon;
    }

    let tip: Vec<u16> = "GoldenDict 划词助手"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let n = tip.len().min(nid.szTip.len());
    nid.szTip[..n].copy_from_slice(&tip[..n]);

    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    log_line("tray icon added");
}

unsafe fn tray_delete(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn open_config() {
    let _ = Command::new("notepad.exe").arg(config_path()).spawn();
}

unsafe fn open_goldendict() {
    let cfg = cfg();
    let _ = Command::new(&cfg.goldendict_path).spawn();
}

unsafe fn reload_config(hwnd: HWND) {
    let cfg = load_config();
    let ok = register_hotkey(hwnd, &cfg);
    update_cfg(cfg.clone());

    let msg: Vec<u16> = if ok {
        format!("配置已重载。\n当前热键：{}", cfg.hotkey)
    } else {
        format!("配置已重载，但热键注册失败：{}\n请检查是否被其他程序占用。", cfg.hotkey)
    }
    .encode_utf16()
    .chain(std::iter::once(0))
    .collect();

    MessageBoxW(HWND::default(), windows::core::PCWSTR(msg.as_ptr()), w!("GoldenDict 划词助手"), MB_OK);
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };

    let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_GOLDENDICT, w!("打开 GoldenDict"));
    let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, w!("打开配置文件"));
    let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("重载配置"));
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("退出"));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);

    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);

    if cmd.0 != 0 {
        let _ = PostMessageW(hwnd, WM_COMMAND, WPARAM(cmd.0 as usize), LPARAM(0));
    }
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            translate_selection();
            LRESULT(0)
        }
        WM_TRAY => {
            let event = (lparam.0 as u32) & 0xFFFF;
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                show_tray_menu(hwnd);
            } else if event == WM_LBUTTONDBLCLK {
                translate_selection();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match (wparam.0 & 0xFFFF) as usize {
                ID_OPEN_GOLDENDICT => open_goldendict(),
                ID_OPEN_CONFIG => open_config(),
                ID_RELOAD => reload_config(hwnd),
                ID_EXIT => {
                    tray_delete(hwnd);
                    PostQuitMessage(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            tray_delete(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let hinst: HINSTANCE = GetModuleHandleW(None).expect("GetModuleHandleW").into();

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            lpszClassName: w!("GdUiaHotkey"),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("GdUiaHotkey"),
            w!("GoldenDict 划词助手"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND::default(),
            None,
            hinst,
            None,
        )
        .expect("CreateWindowExW");

        let cfg = load_config();
        log_line(&format!("startup config: {cfg:?}"));
        let _ = CONFIG.set(Mutex::new(cfg.clone()));

        if !register_hotkey(hwnd, &cfg) {
            let msg = format!(
                "热键注册失败：{}\n\n请检查是否被其他程序占用，或修改配置文件后重载。\n配置文件：{}",
                cfg.hotkey,
                config_path().display()
            );
            let wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
            MessageBoxW(
                HWND::default(),
                windows::core::PCWSTR(wide.as_ptr()),
                w!("GoldenDict 划词助手"),
                MB_ICONERROR | MB_OK,
            );
        }

        tray_add(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        CoUninitialize();
    }
}
