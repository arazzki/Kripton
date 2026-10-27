//! Sprint 6: native Win32 notification-area icon.
//!
//! Slint 1.18 does not expose a tray API, so Kripton uses a hidden message
//! window, `Shell_NotifyIconW`, and `TrackPopupMenu`. Menu actions are queued;
//! the UI thread drains them on its normal 16 ms tick and is the only thread
//! that talks to Slint.

use std::{
    collections::VecDeque,
    ffi::c_void,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW,
                PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
                SetForegroundWindow, TrackPopupMenu, TranslateMessage, IDI_APPLICATION, MF_CHECKED,
                MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY,
                WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
            },
        },
    },
};

const WM_TRAY_CALLBACK: u32 = WM_APP + 1;

const MENU_TOGGLE: usize = 1001;
const MENU_AUTOSTART: usize = 1002;
const MENU_LYRICS: usize = 1003;
const MENU_WATER_1500: usize = 1100;
const MENU_WATER_2000: usize = 1101;
const MENU_WATER_2500: usize = 1102;
const MENU_WATER_3000: usize = 1103;
const MENU_QUIT: usize = 1004;

static TRAY: OnceLock<Arc<TrayShared>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleVisibility,
    SetAutostart(bool),
    SetLyrics(bool),
    SetWaterGoal(u32),
    Quit,
}

struct TrayShared {
    actions: Mutex<VecDeque<TrayAction>>,
    visible: AtomicBool,
    lyrics: AtomicBool,
    water_goal: AtomicU32,
    hwnd: AtomicIsize,
    taskbar_created: u32,
}

impl TrayShared {
    fn push(&self, action: TrayAction) {
        self.actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(action);
    }
}

#[derive(Clone)]
pub struct TrayController {
    shared: Arc<TrayShared>,
}

impl TrayController {
    pub fn start(visible: bool, lyrics: bool, water_goal: u32) -> Self {
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let shared = Arc::new(TrayShared {
            actions: Mutex::new(VecDeque::new()),
            visible: AtomicBool::new(visible),
            lyrics: AtomicBool::new(lyrics),
            water_goal: AtomicU32::new(water_goal),
            hwnd: AtomicIsize::new(0),
            taskbar_created,
        });
        let _ = TRAY.set(shared.clone());

        let thread_shared = shared.clone();
        if let Err(error) = thread::Builder::new()
            .name("kripton-tray".into())
            .spawn(move || unsafe { tray_thread(thread_shared) })
        {
            crate::log_line(&format!("failed to spawn tray thread: {error}"));
        }

        Self { shared }
    }

    pub fn take_actions(&self) -> Vec<TrayAction> {
        let mut actions = self
            .shared
            .actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        actions.drain(..).collect()
    }

    pub fn set_visible(&self, visible: bool) {
        self.shared.visible.store(visible, Ordering::SeqCst);
    }

    pub fn set_lyrics(&self, enabled: bool) {
        self.shared.lyrics.store(enabled, Ordering::SeqCst);
    }

    pub fn set_water_goal(&self, goal: u32) {
        self.shared.water_goal.store(goal, Ordering::SeqCst);
    }

    /// Remove the icon before the process exits. Without `NIM_DELETE` the
    /// notification area can keep a ghost icon until the next hover.
    pub fn shutdown(&self) {
        let hwnd = self.shared.hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            let window = HWND(hwnd as *mut c_void);
            unsafe {
                remove_icon(window);
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

unsafe fn tray_thread(shared: Arc<TrayShared>) {
    let Ok(module) = GetModuleHandleW(None) else {
        crate::log_line("tray: GetModuleHandleW failed");
        return;
    };
    let instance = HINSTANCE(module.0);
    let class_name = w!("KriptonTrayWindow");

    let window_class = WNDCLASSW {
        lpfnWndProc: Some(tray_wndproc),
        hInstance: instance,
        lpszClassName: class_name,
        ..Default::default()
    };
    RegisterClassW(&window_class);

    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!("Kripton"),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        None,
        None,
        Some(instance),
        None,
    ) {
        Ok(hwnd) => hwnd,
        Err(error) => {
            crate::log_line(&format!("tray: CreateWindowExW failed: {error}"));
            return;
        }
    };

    shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
    add_icon(hwnd);
    crate::log_line("tray icon registered");

    let mut message = MSG::default();
    while GetMessageW(&mut message, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }

    remove_icon(hwnd);
    crate::log_line("tray thread stopped");
}

unsafe fn add_icon(hwnd: HWND) {
    let module = GetModuleHandleW(None).unwrap_or_default();
    let icon = LoadIconW(Some(HINSTANCE(module.0)), PCWSTR(1 as _))
        .or_else(|_| LoadIconW(None, IDI_APPLICATION))
        .unwrap_or_default();

    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon: icon,
        ..Default::default()
    };

    let tip: Vec<u16> = std::ffi::OsStr::new("Kripton - native notch")
        .encode_wide()
        .take(data.szTip.len() - 1)
        .collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);

    let _ = Shell_NotifyIconW(NIM_ADD, &data);
}

unsafe fn remove_icon(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
}

unsafe fn show_menu(hwnd: HWND, shared: &TrayShared) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };

    let toggle = if shared.visible.load(Ordering::SeqCst) {
        w!("Hide Kripton")
    } else {
        w!("Show Kripton")
    };
    let _ = AppendMenuW(menu, MF_STRING, MENU_TOGGLE, toggle);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

    let autostart = crate::autostart::is_enabled();
    let _ = AppendMenuW(
        menu,
        if autostart {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        },
        MENU_AUTOSTART,
        w!("Start with Windows"),
    );
    let _ = AppendMenuW(
        menu,
        if shared.lyrics.load(Ordering::SeqCst) {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        },
        MENU_LYRICS,
        w!("Lyrics"),
    );

    let goal = shared.water_goal.load(Ordering::SeqCst);
    let water_menu = CreatePopupMenu().ok();
    if let Some(water) = water_menu {
        for (id, label, value) in [
            (MENU_WATER_1500, w!("1,500 ml"), 1500u32),
            (MENU_WATER_2000, w!("2,000 ml"), 2000),
            (MENU_WATER_2500, w!("2,500 ml"), 2500),
            (MENU_WATER_3000, w!("3,000 ml"), 3000),
        ] {
            let flags = if goal == value {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(water, flags, id, label);
        }
        let _ = AppendMenuW(menu, MF_POPUP, water.0 as usize, w!("Water goal"));
    }

    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit Kripton"));

    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, None, hwnd, None);
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    if let Some(water) = water_menu {
        let _ = DestroyMenu(water);
    }
    let _ = DestroyMenu(menu);
}

unsafe fn handle_command(id: usize, shared: &TrayShared) {
    let action = match id {
        MENU_TOGGLE => Some(TrayAction::ToggleVisibility),
        MENU_AUTOSTART => Some(TrayAction::SetAutostart(!crate::autostart::is_enabled())),
        MENU_LYRICS => Some(TrayAction::SetLyrics(!shared.lyrics.load(Ordering::SeqCst))),
        MENU_WATER_1500 => Some(TrayAction::SetWaterGoal(1500)),
        MENU_WATER_2000 => Some(TrayAction::SetWaterGoal(2000)),
        MENU_WATER_2500 => Some(TrayAction::SetWaterGoal(2500)),
        MENU_WATER_3000 => Some(TrayAction::SetWaterGoal(3000)),
        MENU_QUIT => Some(TrayAction::Quit),
        _ => None,
    };

    if let Some(action) = action {
        shared.push(action);
    }
}

unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(shared) = TRAY.get() else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };

    match message {
        WM_TRAY_CALLBACK => {
            match lparam.0 as u32 {
                WM_LBUTTONDBLCLK => shared.push(TrayAction::ToggleVisibility),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd, shared),
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(wparam.0 & 0xFFFF, shared);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_icon(hwnd);
            shared.hwnd.store(0, Ordering::SeqCst);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => {
            if shared.taskbar_created != 0 && message == shared.taskbar_created {
                add_icon(hwnd);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_commands_are_unique() {
        let ids = [
            MENU_TOGGLE,
            MENU_AUTOSTART,
            MENU_LYRICS,
            MENU_WATER_1500,
            MENU_WATER_2000,
            MENU_WATER_2500,
            MENU_WATER_3000,
            MENU_QUIT,
        ];
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[..index].contains(id));
        }
    }
}
