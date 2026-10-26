#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod living;
mod lyrics;
mod media;
mod spring;
mod state;
mod system;
mod wave;
mod window_ext;

use std::{fs::OpenOptions, io::Write, time::Duration};

use chrono::Local;
use slint::{ComponentHandle, Timer, TimerMode};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

use state::{NotchMode, Page, StateStore};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    install_panic_logger();
    log_line("Kripton starting");

    // SMTC (Sprint 2) and OLE drag/drop (Sprint 4) both need a single-threaded
    // apartment. Initialise before winit creates the event loop.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        log_line(&format!("CoInitializeEx failed: {hr:?}"));
    }

    if !window_ext::acquire_single_instance() {
        log_line("another Kripton instance is already running; exiting");
        return Ok(());
    }

    let store = StateStore::load();
    log_line(&format!("state file: {}", store.path().display()));

    let app = AppWindow::new()?;
    apply_persisted_state(&app, &store);
    wire_callbacks(&app, &store);
    update_clock(&app);
    apply_battery(&app, system::read_battery());

    // Both workers publish plain snapshots through shared state, so the UI
    // thread never blocks on COM or audio.
    let media = media::MediaController::start();
    let audio = wave::AudioWave::start();

    window_ext::apply_mode(&app, window_ext::collapsed_mode());
    app.show()?;
    window_ext::install_overlay(&app, media, audio, store.clone());

    let _clock_timer = start_clock(&app);
    let _battery_timer = start_battery(&app);
    log_line("event loop ready");

    let result = app.run();
    let _ = store.save();
    log_line("Kripton stopped");
    result
}

fn apply_persisted_state(app: &AppWindow, store: &StateStore) {
    let page = store.with(|state| state.page());
    app.set_active_page(page.index() as i32);
    app.set_page_title(page.title().into());
    app.set_page_subtitle(page.subtitle().into());
    app.set_lyrics_enabled(store.with(|state| state.lyrics_enabled));
    app.set_build_label(
        format!("v{} · Sprint 3 · Synced Lyrics", env!("CARGO_PKG_VERSION")).into(),
    );
}

fn wire_callbacks(app: &AppWindow, store: &StateStore) {
    let weak_for_toggle = app.as_weak();
    app.on_toggle_expand(move || {
        if let Some(app) = weak_for_toggle.upgrade() {
            let expand = !app.get_expanded();
            if !expand {
                window_ext::lock_hover_until_leave();
            }
            let mode = if expand {
                if app.get_active_page() == 0 && app.get_lyrics_enabled() {
                    NotchMode::Lyrics
                } else {
                    NotchMode::Expanded
                }
            } else {
                window_ext::collapsed_mode()
            };
            window_ext::apply_mode(&app, mode);
        }
    });

    let weak_for_page = app.as_weak();
    let store_for_page = store.clone();
    app.on_page_selected(move |index| {
        let page = Page::from_index(index.clamp(0, 4) as u8);
        store_for_page.update(|state| state.active_page = page.index());

        if let Some(app) = weak_for_page.upgrade() {
            app.set_active_page(page.index() as i32);
            app.set_page_title(page.title().into());
            app.set_page_subtitle(page.subtitle().into());

            if app.get_expanded() {
                let mode = if page == Page::Music && app.get_lyrics_enabled() {
                    NotchMode::Lyrics
                } else {
                    NotchMode::Expanded
                };
                window_ext::apply_mode(&app, mode);
            }
        }
    });

    let store_for_quit = store.clone();
    app.on_quit(move || {
        let _ = store_for_quit.save();
        let _ = slint::quit_event_loop();
    });

    let store_for_close = store.clone();
    app.window().on_close_requested(move || {
        let _ = store_for_close.save();
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
}

fn start_clock(app: &AppWindow) -> Timer {
    let weak = app.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(app) = weak.upgrade() {
            update_clock(&app);
        }
    });
    timer
}

fn update_clock(app: &AppWindow) {
    let now = Local::now();
    app.set_clock_text(now.format("%H:%M").to_string().into());
    app.set_date_text(now.format("%a %d %b %Y").to_string().into());
}

fn start_battery(app: &AppWindow) -> Timer {
    let weak = app.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(30), move || {
        if let Some(app) = weak.upgrade() {
            apply_battery(&app, system::read_battery());
        }
    });
    timer
}

fn apply_battery(app: &AppWindow, status: system::BatteryStatus) {
    app.set_battery_present(status.present);
    app.set_battery_on_ac(status.on_ac);
    app.set_battery_percent(status.percent);
    app.set_battery_charging(status.charging);

    let arc = system::battery_arc(status.percent);
    app.set_battery_arc_x(arc.end_x);
    app.set_battery_arc_y(arc.end_y);
    app.set_battery_arc_large(arc.large);
    app.set_battery_arc_visible(arc.visible && status.present);
    app.set_battery_full(arc.full && status.present);
}

fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        let message = format!("panic: {info}");
        log_line(&message);
        eprintln!("{message}");
    }));
}

pub(crate) fn log_line(message: &str) {
    let Some(directory) = state::state_path().parent().map(|path| path.to_path_buf()) else {
        return;
    };
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("kripton.log"))
    {
        let _ = writeln!(
            file,
            "[{}] {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            message
        );
    }
}
