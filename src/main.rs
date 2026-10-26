#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod state;
mod window_ext;

use std::{fs::OpenOptions, io::Write, time::Duration};

use chrono::Local;
use slint::{ComponentHandle, Timer, TimerMode};

use state::{NotchMode, Page, StateStore};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    install_panic_logger();
    log_line("Kripton starting");

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

    window_ext::apply_mode(&app, NotchMode::Idle);
    app.show()?;
    window_ext::install_overlay(&app);

    let _clock_timer = start_clock(&app);
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
    app.set_build_label(format!("v{} · Sprint 0 · Foundation", env!("CARGO_PKG_VERSION")).into());
}

fn wire_callbacks(app: &AppWindow, store: &StateStore) {
    let weak_for_toggle = app.as_weak();
    app.on_toggle_expand(move || {
        if let Some(app) = weak_for_toggle.upgrade() {
            let expand = !app.get_expanded();
            if !expand {
                window_ext::lock_hover_until_leave();
            }
            window_ext::apply_mode(
                &app,
                if expand {
                    NotchMode::Expanded
                } else {
                    NotchMode::Idle
                },
            );
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

fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        let message = format!("panic: {info}");
        log_line(&message);
        eprintln!("{message}");
    }));
}

fn log_line(message: &str) {
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
