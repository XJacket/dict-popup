//! Regression test for clipboard capture.
//!
//! The bug it locks down: `Ctrl+C` is injected, the application copies, but the
//! reader returns nothing, so the card reports "clipboard unchanged" even when
//! had text selected.
//!
//! The test owns a real window with a real `EDIT` control, selects text in it,
//! gives that window the foreground, and runs the production capture code while
//! this thread pumps messages (a window's messages must be pumped by the thread
//! that created it).
//!
//! ```text
//! cargo test --release --test clipboard_loop -- --nocapture
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
    RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowTextW, TranslateMessage,
    PM_REMOVE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE,
};

const EM_SETSEL: u32 = 0x00B1;

struct TestWindow {
    hwnd: HWND,
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: we own the window and destroy it exactly once.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    // SAFETY: default handling is correct for this throwaway window.
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn create_window(word: &str) -> TestWindow {
    // SAFETY: single-threaded set-up; every handle is checked before use.
    unsafe {
        let instance = GetModuleHandleW(None).expect("module handle");
        let class_name = w!("DictInputCaptureLoopTest");

        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("capture loop test"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            80,
            80,
            360,
            160,
            HWND::default(),
            None,
            instance,
            None,
        )
        .expect("main window");

        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0004), // ES_MULTILINE
            8,
            8,
            320,
            80,
            hwnd,
            None,
            instance,
            None,
        )
        .expect("edit control");

        let wide: Vec<u16> = word.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetWindowTextW(edit, windows::core::PCWSTR(wide.as_ptr()));
        SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
        let _ = SetFocus(edit);
        let _ = SetForegroundWindow(hwnd);

        TestWindow { hwnd }
    }
}

#[ignore = "needs exclusive foreground; clipboard tools on this machine steal it"]
#[test]
fn clipboard_capture_reads_a_real_selection() {
    // Seed the clipboard BEFORE creating the window: writing to it can wake a
    // clipboard tool that steals the foreground.
    dict_input::debug_clipboard_write("SENTINEL");
    let window = create_window("apple");

    let done = Arc::new(AtomicBool::new(false));
    let capture_done = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        // Let the window come up and take the foreground first.
        std::thread::sleep(std::time::Duration::from_millis(400));
        // Make sure stale clipboard text cannot masquerade as the selection.
        dict_input::debug_clipboard_write("SENTINEL");
        let result = dict_input::debug_capture_selection();
        capture_done.store(true, Ordering::Release);
        result
    });

    // Pump this thread's messages until the capture finishes; without this the
    // edit control never handles WM_KEYDOWN and nothing is copied.
    // SAFETY: standard PeekMessage pump on the thread that owns the window.
    unsafe {
        let mut msg = MSG::default();
        while !done.load(Ordering::Acquire) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let result = worker.join().expect("capture thread");
    println!("window 0x{:X}", window.hwnd.0 as isize);
    println!("capture result: {result:?}");

    match result {
        Ok(text) => assert_eq!(text.trim(), "apple", "captured the wrong text"),
        Err(error) => panic!("capture failed: {error}"),
    }
}




/// Same as the test above, but first presses the real hotkey chord
/// (Ctrl+Alt+D) exactly as the user does. If the chord itself disturbs focus or
/// the selection, this is where the user-visible bug reproduces.
#[ignore = "needs exclusive foreground; clipboard tools on this machine steal it"]
#[test]
fn clipboard_capture_survives_the_hotkey_press() {
    // Seed the clipboard BEFORE creating the window: writing to it can wake a
    // clipboard tool that steals the foreground.
    dict_input::debug_clipboard_write("SENTINEL");
    let window = create_window("apple");

    let done = Arc::new(AtomicBool::new(false));
    let capture_done = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        // Make sure stale clipboard text cannot masquerade as the selection.
        dict_input::debug_clipboard_write("SENTINEL");
        // VK_CONTROL, VK_MENU, 'D'
        dict_input::debug_press_combo(&[0x11, 0x12, 0x44]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let result = dict_input::debug_capture_selection();
        capture_done.store(true, Ordering::Release);
        result
    });

    // SAFETY: standard PeekMessage pump on the thread that owns the window.
    unsafe {
        let mut msg = MSG::default();
        while !done.load(Ordering::Acquire) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let result = worker.join().expect("capture thread");
    println!("window 0x{:X}", window.hwnd.0 as isize);
    println!("capture after hotkey result: {result:?}");

    match result {
        Ok(text) => assert_eq!(text.trim(), "apple", "captured the wrong text"),
        Err(error) => panic!("capture after hotkey failed: {error}"),
    }
}



