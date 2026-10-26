use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    rc::Rc,
    time::{Duration, Instant},
};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use windows::{
    core::w,
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, POINT, RECT},
        Graphics::{
            Dwm::{
                DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
                DWM_WINDOW_CORNER_PREFERENCE,
            },
            Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY},
        },
        System::Threading::CreateMutexW,
        UI::WindowsAndMessaging::{
            GetCursorPos, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, SetWindowPos,
            GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        },
    },
};

use crate::{state::NotchMode, AppWindow};

const HOVER_OPEN_DELAY: Duration = Duration::from_millis(60);
const HOVER_CLOSE_DELAY: Duration = Duration::from_millis(250);
const HOVER_POLL_INTERVAL: Duration = Duration::from_millis(16);
const HOVER_PADDING: f32 = 6.0;

thread_local! {
    /// Set after the user explicitly collapses the notch with a click. Hover
    /// re-expansion stays disabled until the pointer leaves the notch once.
    static HOVER_LOCKED_UNTIL_LEAVE: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy)]
pub struct OverlayWindow {
    hwnd: HWND,
}

impl OverlayWindow {
    pub fn attach(app: &AppWindow) -> Option<Self> {
        let window_handle = app.window().window_handle();
        let raw_handle = window_handle.window_handle().ok()?;
        let RawWindowHandle::Win32(win32) = raw_handle.as_raw() else {
            return None;
        };
        let hwnd = HWND(win32.hwnd.get() as *mut c_void);

        unsafe {
            let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let wanted = current | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;
            if wanted != current {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted as isize);
            }

            // Windows 11 rounds popup corners by default, which would visibly
            // cut the notch canvas. Keep the native window square.
            let corner = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const DWM_WINDOW_CORNER_PREFERENCE as *const c_void,
                size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
        }

        let overlay = Self { hwnd };
        overlay.reposition();
        overlay.set_click_through(true);
        Some(overlay)
    }

    pub fn reposition(&self) {
        let mut window_rect = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut window_rect) }.is_err() {
            return;
        }
        let width = window_rect.right - window_rect.left;

        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        if !unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool() {
            return;
        }

        let x = monitor_info.rcMonitor.left
            + (monitor_info.rcMonitor.right - monitor_info.rcMonitor.left - width) / 2;
        let y = monitor_info.rcMonitor.top;

        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
    }

    pub fn set_click_through(&self, enabled: bool) {
        unsafe {
            let current = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            let wanted = if enabled {
                current | WS_EX_TRANSPARENT.0
            } else {
                current & !WS_EX_TRANSPARENT.0
            };
            if wanted != current {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, wanted as isize);
            }
        }
    }

    fn cursor_inside_notch(&self, app: &AppWindow) -> bool {
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_err() {
            return false;
        }

        let mut window_rect = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut window_rect) }.is_err() {
            return false;
        }

        let scale = app.window().scale_factor();
        let notch_width = app.get_notch_width() * scale;
        let notch_height = app.get_notch_height() * scale;
        let window_width = (window_rect.right - window_rect.left) as f32;
        let left = window_rect.left as f32 + (window_width - notch_width) * 0.5;
        let top = window_rect.top as f32;
        let padding = HOVER_PADDING * scale;

        let x = cursor.x as f32;
        let y = cursor.y as f32;
        x >= left - padding
            && x <= left + notch_width + padding
            && y >= top
            && y <= top + notch_height + padding
    }
}

pub fn apply_mode(app: &AppWindow, mode: NotchMode) {
    let (width, height) = mode.size();
    app.set_notch_width(width);
    app.set_notch_height(height);
    app.set_expanded(matches!(
        mode,
        NotchMode::Expanded | NotchMode::Lyrics | NotchMode::Hud
    ));
}

pub fn install_overlay(app: &AppWindow) {
    try_attach(app.as_weak(), 0);
}

fn try_attach(app: Weak<AppWindow>, attempts: u32) {
    Timer::single_shot(Duration::from_millis(16), move || {
        let Some(app) = app.upgrade() else {
            return;
        };

        if let Some(overlay) = OverlayWindow::attach(&app) {
            install_hover_controller(&app, overlay);
        } else if attempts < 120 {
            try_attach(app.as_weak(), attempts + 1);
        } else {
            eprintln!("[Kripton] native window handle was not available after 2 seconds");
        }
    });
}

fn install_hover_controller(app: &AppWindow, overlay: OverlayWindow) {
    struct HoverRuntime {
        entered_at: Option<Instant>,
        left_at: Option<Instant>,
        click_through: bool,
    }

    let weak = app.as_weak();
    let runtime = Rc::new(RefCell::new(HoverRuntime {
        entered_at: None,
        left_at: None,
        click_through: true,
    }));

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, HOVER_POLL_INTERVAL, move || {
        let Some(app) = weak.upgrade() else {
            return;
        };

        let inside = overlay.cursor_inside_notch(&app);
        let now = Instant::now();
        let mut runtime = runtime.borrow_mut();

        let should_click_through = !inside;
        if runtime.click_through != should_click_through {
            overlay.set_click_through(should_click_through);
            runtime.click_through = should_click_through;
        }

        if inside {
            runtime.left_at = None;
            if HOVER_LOCKED_UNTIL_LEAVE.with(Cell::get) {
                runtime.entered_at = None;
                return;
            }

            if !app.get_expanded() {
                let entered = *runtime.entered_at.get_or_insert(now);
                if now.duration_since(entered) >= HOVER_OPEN_DELAY {
                    apply_mode(&app, NotchMode::Expanded);
                    runtime.entered_at = None;
                }
            } else {
                runtime.entered_at = None;
            }
        } else {
            HOVER_LOCKED_UNTIL_LEAVE.with(|locked| locked.set(false));
            runtime.entered_at = None;

            if app.get_expanded() {
                let left = *runtime.left_at.get_or_insert(now);
                if now.duration_since(left) >= HOVER_CLOSE_DELAY {
                    apply_mode(&app, NotchMode::Idle);
                    runtime.left_at = None;
                }
            } else {
                runtime.left_at = None;
            }
        }
    });

    // The overlay needs its hover loop for the whole process lifetime. Leaking
    // this handle is intentional; dropping it would silently stop the loop.
    std::mem::forget(timer);
}

pub fn lock_hover_until_leave() {
    HOVER_LOCKED_UNTIL_LEAVE.with(|locked| locked.set(true));
}

/// Returns true when this process owns the single-instance mutex. The mutex
/// handle is intentionally kept open until the process exits.
pub fn acquire_single_instance() -> bool {
    let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\Kripton.SingleInstance")) };
    unsafe { GetLastError() != ERROR_ALREADY_EXISTS }
}
