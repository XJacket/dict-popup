//! Where the hotkey was pressed.
//!
//! An [`Anchor`] is sampled exactly once, on the host thread, in the instant
//! `WM_HOTKEY` is handled. `dict-popup` positions the card from it when the
//! capture result arrives - up to one capture budget later - so the UI never
//! has to ask the cursor, the monitor or the DPI again after the fact. That
//! removes the second DPI source (`pixels_per_point()` in the render pass) that
//! could disagree with the monitor scale on mixed-DPI setups.
//!
//! Win32 lives here, next to the host plumbing; the types themselves are plain
//! data and are re-exported from [`crate::host`].

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// A monitor's usable rectangle (taskbar excluded), in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WorkArea {
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }

    /// A monitor that reports an empty work area cannot be placed on.
    fn is_usable(&self) -> bool {
        self.width() > 0 && self.height() > 0
    }
}

/// The hotkey's anchor: everything the card needs to place itself.
///
/// All coordinates are physical pixels in virtual-screen space, which is what
/// Win32 reports for a `PER_MONITOR_AWARE_V2` process (the host sets that
/// before any window exists).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// `GetCursorPos()` when the hotkey fired.
    pub cursor: Option<(i32, i32)>,
    /// `MONITORINFO::rcWork` of the monitor containing the cursor.
    pub work_area: Option<WorkArea>,
    /// That monitor's effective scale factor (1.0 = 96 DPI).
    pub scale: f32,
}

impl Default for Anchor {
    /// Unknown anchor: no cursor, no work area, scale 1.0. The card then falls
    /// back to a fixed position instead of guessing.
    fn default() -> Self {
        Self {
            cursor: None,
            work_area: None,
            scale: 1.0,
        }
    }
}

impl Anchor {
    /// The anchor's scale factor, sanitised for layout maths.
    ///
    /// A missing or nonsensical DPI (0, NaN, infinity) degrades to 1.0 rather
    /// than producing NaN positions.
    pub fn effective_scale(&self) -> f32 {
        if self.scale.is_finite() && self.scale > 0.1 {
            self.scale
        } else {
            1.0
        }
    }

    /// Sample the cursor and the monitor under it.
    ///
    /// The host calls this on the host thread when `WM_HOTKEY` arrives. The
    /// only other caller is `dict-popup --demo`, which opens the card without
    /// a hotkey at all.
    ///
    /// Never fails: a failed query leaves that part of the anchor `None` (or
    /// the scale at 1.0) and the card degrades to a fixed position.
    pub fn sample() -> Self {
        let mut anchor = Anchor::default();
        let mut point = POINT::default();

        // SAFETY: `point` is a valid, writable POINT that outlives the call;
        // GetCursorPos only reads the cursor and needs no cleanup.
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            return anchor;
        }
        anchor.cursor = Some((point.x, point.y));

        // SAFETY: read-only monitor lookup. MONITOR_DEFAULTTONEAREST makes it
        // total (it falls back to the nearest monitor); the only failure mode
        // is a null handle, which is checked below.
        let monitor =
            unsafe { MonitorFromPoint(POINT { x: point.x, y: point.y }, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_invalid() {
            return anchor;
        }

        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is fully initialised with the `cbSize` the API
        // requires and outlives the call; GetMonitorInfoW only writes into it.
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let area = WorkArea {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            };
            if area.is_usable() {
                anchor.work_area = Some(area);
            }
        }

        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        // SAFETY: both out-parameters are valid for the duration of the call;
        // a failure leaves the 1.0 fallback in place.
        let dpi = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        if dpi.is_ok() && dpi_x > 0 {
            anchor.scale = dpi_x as f32 / 96.0;
        }

        anchor
    }
}
