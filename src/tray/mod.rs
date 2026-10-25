use std::mem::zeroed;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TrackPopupMenu, HICON, HMENU, IDI_APPLICATION, MF_SEPARATOR, MF_STRING,
    MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const WM_TRAYICON: u32 = WM_APP + 101;
const IDM_TOGGLE: usize = 2001;
const IDM_OPEN_SHELF: usize = 2002;
const IDM_ABOUT: usize = 2003;
const IDM_EXIT: usize = 2004;

pub struct SystemTray;

impl SystemTray {
    pub fn start(weak_app: slint::Weak<crate::AppWindow>) {
        std::thread::Builder::new()
            .name("kripton-system-tray".into())
            .spawn(move || unsafe {
                run_tray_message_loop(weak_app);
            })
            .expect("Failed to spawn system tray thread");
    }
}

unsafe fn run_tray_message_loop(weak_app: slint::Weak<crate::AppWindow>) {
    let class_name = w!("KriptonTrayWindowClass");

    let wnd_class = WNDCLASSW {
        lpfnWndProc: Some(tray_wnd_proc),
        lpszClassName: class_name,
        ..zeroed()
    };

    RegisterClassW(&wnd_class);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!("KriptonTrayWindow"),
        WS_OVERLAPPEDWINDOW,
        0,
        0,
        0,
        0,
        None,
        None,
        None,
        None,
    );

    if hwnd.is_err() {
        eprintln!("[Kripton] Failed to create tray message window");
        return;
    }
    let hwnd = hwnd.unwrap();

    let icon: HICON = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();

    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: icon,
        szTip: [0u16; 128],
        ..zeroed()
    };

    // Tooltip: "Kripton - Windows Dynamic Island"
    let tip = "Kripton - Dynamic Island";
    for (i, c) in tip.encode_utf16().enumerate() {
        if i < 127 {
            nid.szTip[i] = c;
        }
    }

    let _ = Shell_NotifyIconW(NIM_ADD, &nid);

    // Store weak_app pointer in thread-safe OnceLock
    let _ = TRAY_APP.set(weak_app);

    let mut msg: MSG = zeroed();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        DispatchMessageW(&msg);
    }

    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    let _ = DestroyWindow(hwnd);
}

static TRAY_APP: std::sync::OnceLock<slint::Weak<crate::AppWindow>> = std::sync::OnceLock::new();

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            let event = lparam.0 as u32;
            if event == WM_LBUTTONUP {
                // Left click: Toggle island expansion
                if let Some(weak) = TRAY_APP.get() {
                    let _ = weak.upgrade_in_event_loop(|app| {
                        app.set_is_expanded(!app.get_is_expanded());
                    });
                }
            } else if event == WM_RBUTTONUP {
                // Right click: Show context menu
                show_tray_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd_id = wparam.0 as usize;
            match cmd_id {
                IDM_TOGGLE => {
                    if let Some(weak) = TRAY_APP.get() {
                        let _ = weak.upgrade_in_event_loop(|app| {
                            app.set_is_expanded(!app.get_is_expanded());
                        });
                    }
                }
                IDM_OPEN_SHELF => {
                    let user_profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Public".into());
                    let pictures_dir = std::path::PathBuf::from(user_profile).join("Pictures").join("Screenshots").join("Kripton");
                    let _ = std::process::Command::new("explorer").arg(&pictures_dir).spawn();
                }
                IDM_ABOUT => {
                    if let Some(weak) = TRAY_APP.get() {
                        let _ = weak.upgrade_in_event_loop(|app| {
                            app.set_hud_icon("🏝️".into());
                            app.set_hud_title("Kripton v0.1.0".into());
                            app.set_hud_subtitle("Native Rust Dynamic Island".into());
                            app.set_hud_show_bar(false);
                            app.set_is_hud_active(true);
                        });
                    }
                }
                IDM_EXIT => {
                    let _ = slint::quit_event_loop();
                    PostQuitMessage(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY | WM_CLOSE => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu: HMENU = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };

    let _ = AppendMenuW(menu, MF_STRING, IDM_TOGGLE, w!("Toggle Dynamic Island"));
    let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN_SHELF, w!("Open Screenshots Shelf"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, IDM_ABOUT, w!("About Kripton..."));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("Exit Kripton"));

    let mut pt = POINT { x: 0, y: 0 };
    let _ = GetCursorPos(&mut pt);

    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN,
        pt.x,
        pt.y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
}
