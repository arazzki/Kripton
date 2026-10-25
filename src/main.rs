#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod media;
mod shelf;
mod system;
mod tray;

slint::include_modules!();

#[repr(C)]
struct SystemTime {
    w_year: u16,
    w_month: u16,
    w_day_of_week: u16,
    w_day: u16,
    w_hour: u16,
    w_minute: u16,
    w_second: u16,
    w_milliseconds: u16,
}

extern "system" {
    fn GetLocalTime(lp_system_time: *mut SystemTime);
}

fn get_local_time_string() -> String {
    let mut st = std::mem::MaybeUninit::<SystemTime>::uninit();
    unsafe {
        GetLocalTime(st.as_mut_ptr());
        let st = st.assume_init();
        format!("{:02}:{:02}", st.w_hour, st.w_minute)
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;

    // 1. Center the Dynamic Island at the top of the primary monitor
    let screen_width = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
        )
    };
    let window_width = 640.0;
    let x = ((screen_width as f32) - window_width) / 2.0;
    app.window().set_position(slint::LogicalPosition::new(x, 0.0));

    // 2. Set initial clock time
    app.set_clock_text(get_local_time_string().into());

    // 3. Start WinRT SMTC Universal Media Controller (Sprint 2)
    let media_handle = media::MediaController::start(app.as_weak());

    // 4. Start Windows System HUD Sensor: WASAPI Volume & Battery (Sprint 3)
    system::SystemHudManager::start(app.as_weak());

    // 5. Start Screenshot & Quick Shelf Manager (Sprint 4)
    let shelf_handle = shelf::ShelfManager::start(app.as_weak());

    // 6. Start Windows Taskbar System Tray Icon (Sprint 5)
    tray::SystemTray::start(app.as_weak());

    // 7. Hook UI Media Action Callbacks
    let handle_play = media_handle.clone();
    app.on_toggle_play_pause(move || {
        handle_play.send(media::MediaCommand::TogglePlayPause);
    });

    let handle_next = media_handle.clone();
    app.on_next_track(move || {
        handle_next.send(media::MediaCommand::Next);
    });

    let handle_prev = media_handle;
    app.on_prev_track(move || {
        handle_prev.send(media::MediaCommand::Previous);
    });

    // 8. Hook UI Shelf Callbacks
    let shelf_open_h = shelf_handle.clone();
    app.on_shelf_open(move || {
        shelf_open_h.send(shelf::ShelfAction::OpenLatest);
    });

    let shelf_copy_h = shelf_handle.clone();
    app.on_shelf_copy_path(move || {
        shelf_copy_h.send(shelf::ShelfAction::CopyPathLatest);
    });

    let shelf_dismiss_h = shelf_handle;
    app.on_shelf_dismiss(move || {
        shelf_dismiss_h.send(shelf::ShelfAction::Dismiss);
    });

    // 9. Procedural Sine-Wave Equalizer Timer (runs only when playing)
    let weak_eq = app.as_weak();
    let mut tick: f32 = 0.0;
    let eq_timer = slint::Timer::default();
    eq_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(120),
        move || {
            if let Some(app) = weak_eq.upgrade() {
                if app.get_is_playing() {
                    tick += 0.35;
                    let b1 = 5.0 + 8.0 * (tick * 1.3).sin().abs();
                    let b2 = 5.0 + 13.0 * (tick * 1.8 + 0.5).sin().abs();
                    let b3 = 5.0 + 11.0 * (tick * 1.1 + 1.2).sin().abs();
                    let b4 = 5.0 + 9.0 * (tick * 2.1 + 0.8).sin().abs();
                    app.set_bar1_height(b1);
                    app.set_bar2_height(b2);
                    app.set_bar3_height(b3);
                    app.set_bar4_height(b4);
                } else {
                    app.set_bar1_height(4.0);
                    app.set_bar2_height(4.0);
                    app.set_bar3_height(4.0);
                    app.set_bar4_height(4.0);
                }
            }
        },
    );

    // 10. Clock Update Timer (1 second interval)
    let weak_clock = app.as_weak();
    let clock_timer = slint::Timer::default();
    clock_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(1),
        move || {
            if let Some(app) = weak_clock.upgrade() {
                app.set_clock_text(get_local_time_string().into());
            }
        },
    );

    println!("[Kripton] Native Dynamic Island Running on Windows.");
    app.run()
}
