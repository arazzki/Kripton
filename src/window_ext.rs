use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel, Weak};
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
        UI::{
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                GetCursorPos, GetWindowLongPtrW, GetWindowRect, SetForegroundWindow,
                SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOPMOST,
                SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
                WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
            },
        },
    },
};

use crate::{
    focus::{self, FocusPhase, FocusTimer},
    headphone::HeadphoneWatcher,
    hud::{self, HudContent, HudKind, HudState, PowerSnapshot},
    living::{Living, LivingFrame},
    media::{self, Artwork, MediaController},
    shelf::{self, ShelfManager, ShelfThumb},
    spring::{Spring, SpringConfig},
    state::{NotchMode, Page, TodoItem},
    todo,
    tray::{TrayAction, TrayController},
    water::{self, WaterTracker},
    wave::AudioWave,
    AppWindow, ShelfView, TodoView,
};

const HOVER_CLOSE_DELAY: Duration = Duration::from_millis(350);
const HOVER_POLL_INTERVAL: Duration = Duration::from_millis(16);
pub const HIT_MARGIN: f32 = 12.0;
pub const WAKE_STRIP_W: f32 = 200.0;
pub const WAKE_STRIP_H: f32 = 6.0;
const LIVING_EVERY: u64 = 3; // Living properties update at ~20 fps.
const MEDIA_POLL_INTERVAL: Duration = Duration::from_millis(250);
const POWER_POLL_INTERVAL: Duration = Duration::from_millis(250);
const COLLAPSE_REVEAL_WIDTH: f32 = 380.0;

thread_local! {
    /// Set after the user explicitly collapses the notch with a click. Hover
    /// re-expansion stays disabled until the pointer leaves the notch once.
    static HOVER_LOCKED_UNTIL_LEAVE: Cell<bool> = const { Cell::new(false) };
    /// Whether the default render endpoint currently has audible output.
    static MEDIA_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Animated notch geometry. Shared by every mode transition so a target
    /// change mid-flight reverses the spring instead of snapping.
    static MORPH: RefCell<Morph> = RefCell::new(Morph::new());
}

struct Morph {
    width: Spring,
    height: Spring,
    config: SpringConfig,
    /// A collapse requested while the notch is still wide. The expanded shell
    /// stays visible until the spring is close to compact size.
    pending_collapse: Option<NotchMode>,
}

impl Morph {
    fn new() -> Self {
        let (width, height) = NotchMode::Stealth.size();
        Self {
            width: Spring::new(width),
            height: Spring::new(height),
            config: SpringConfig::default(),
            pending_collapse: None,
        }
    }
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
            let wanted = current | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0;
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

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Tray show/hide. Win32 `ShowWindow` is used instead of Slint's
    /// `ComponentHandle::hide`, because Slint releases its event-loop keepalive
    /// when the last window is hidden and the tray would stop working.
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
        if visible {
            self.reposition();
        }
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

    pub fn set_activatable(&self, enabled: bool) {
        unsafe {
            let current = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            let wanted = if enabled {
                current & !WS_EX_NOACTIVATE.0
            } else {
                current | WS_EX_NOACTIVATE.0
            };
            if wanted != current {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, wanted as isize);
            }
            if enabled {
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(Some(self.hwnd));
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
        let window_width = (window_rect.right - window_rect.left) as f32;
        let notch_width = app.get_notch_width() * scale;
        let notch_height = app.get_notch_height() * scale;

        let x = (cursor.x - window_rect.left) as f32;
        let y = (cursor.y - window_rect.top) as f32;

        let is_awake = notch_height > (10.0 * scale);
        let strip_w = WAKE_STRIP_W * scale;
        let strip_h = WAKE_STRIP_H * scale;
        let strip_x0 = (window_width - strip_w) * 0.5;
        let on_wake_strip =
            x >= strip_x0 && x <= strip_x0 + strip_w && y >= -4.0 * scale && y <= strip_h;

        if is_awake {
            let hit_margin = HIT_MARGIN * scale;
            let rect_x = (window_width - notch_width) * 0.5;
            let on_island = x >= rect_x - hit_margin
                && x <= rect_x + notch_width + hit_margin
                && y >= -4.0 * scale
                && y <= notch_height + hit_margin;
            on_island || on_wake_strip
        } else {
            on_wake_strip
        }
    }

    /// Cursor position relative to the centre of the notch, in logical pixels.
    /// The living model uses this for the ferrofluid pull.
    fn cursor_relative_to_notch(&self, app: &AppWindow) -> Option<(f32, f32)> {
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_err() {
            return None;
        }

        let mut window_rect = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut window_rect) }.is_err() {
            return None;
        }

        let scale = app.window().scale_factor();
        let notch_height = app.get_notch_height() * scale;
        let window_width = (window_rect.right - window_rect.left) as f32;
        let centre_x = window_rect.left as f32 + window_width * 0.5;
        let centre_y = window_rect.top as f32 + notch_height * 0.5;

        Some((
            (cursor.x as f32 - centre_x) / scale,
            (cursor.y as f32 - centre_y) / scale,
        ))
    }
}

pub fn apply_mode(app: &AppWindow, mode: NotchMode) {
    let (width, height) = mode.size();
    let expanded = matches!(mode, NotchMode::Expanded | NotchMode::Lyrics);
    let hud = mode.is_hud();
    let stealth = matches!(mode, NotchMode::Stealth);

    MORPH.with(|morph| {
        let mut morph = morph.borrow_mut();
        morph.width.set_target(width);
        morph.height.set_target(height);

        if hud {
            morph.pending_collapse = None;
            app.set_stealth(false);
            app.set_expanded(false);
            app.set_closed(false);
            app.set_hud_visible(true);
        } else if expanded {
            morph.pending_collapse = None;
            app.set_stealth(false);
            app.set_expanded(true);
            app.set_closed(false);
            app.set_hud_visible(false);
        } else if app.get_expanded() && morph.width.value() > COLLAPSE_REVEAL_WIDTH {
            // Keep the expanded shell visible until the black shape has shrunk
            // close to compact size, then swap content in the runtime tick.
            morph.pending_collapse = Some(mode);
        } else {
            morph.pending_collapse = None;
            app.set_stealth(stealth);
            app.set_expanded(false);
            let show_closed = matches!(mode, NotchMode::Closed | NotchMode::CompactHovered);
            app.set_closed(show_closed);
            app.set_hud_visible(false);
        }
    });
}

/// Baseline mode when inactive: Stealth mode (184x5 px).
pub fn collapsed_mode() -> NotchMode {
    NotchMode::Stealth
}

pub fn install_overlay(
    app: &AppWindow,
    media: MediaController,
    audio: AudioWave,
    shelf: ShelfManager,
    store: crate::state::StateStore,
    tray: TrayController,
    headphone: HeadphoneWatcher,
) {
    try_attach(
        app.as_weak(),
        0,
        media,
        Arc::new(audio),
        shelf,
        store,
        tray,
        headphone,
    );
}

#[allow(clippy::too_many_arguments)]
fn try_attach(
    app: Weak<AppWindow>,
    attempts: u32,
    media: MediaController,
    audio: Arc<AudioWave>,
    shelf: ShelfManager,
    store: crate::state::StateStore,
    tray: TrayController,
    headphone: HeadphoneWatcher,
) {
    Timer::single_shot(Duration::from_millis(16), move || {
        let Some(app) = app.upgrade() else {
            return;
        };

        if let Some(overlay) = OverlayWindow::attach(&app) {
            install_runtime(&app, overlay, media, audio, shelf, store, tray, headphone);
        } else if attempts < 120 {
            try_attach(
                app.as_weak(),
                attempts + 1,
                media.clone(),
                audio.clone(),
                shelf.clone(),
                store,
                tray.clone(),
                headphone.clone(),
            );
        } else {
            eprintln!("[Kripton] native window handle was not available after 2 seconds");
        }
    });
}

struct PlaybackClock {
    position_ms: u64,
    duration_ms: u64,
    playing: bool,
    synced_at: Instant,
}

impl PlaybackClock {
    fn position(&self, now: Instant) -> u64 {
        if !self.playing {
            return self.position_ms;
        }
        let elapsed = now.saturating_duration_since(self.synced_at).as_millis() as u64;
        let projected = self.position_ms.saturating_add(elapsed);
        if self.duration_ms > 0 {
            projected.min(self.duration_ms)
        } else {
            projected
        }
    }
}

struct Runtime {
    left_at: Option<Instant>,
    click_through: bool,
    last_tick: Instant,
    last_living: Instant,
    last_media_poll: Instant,
    ticks: u64,
    media_active: bool,
    media: MediaController,
    audio: Arc<AudioWave>,
    playing: bool,
    track_generation: u64,
    playback: PlaybackClock,
    elapsed_seconds: u64,
    lyrics: crate::lyrics::LyricsManager,
    current_lyrics: Option<crate::lyrics::SyncedLyrics>,
    last_lyrics_track: String,
    last_synced_line: Option<String>,
    store: crate::state::StateStore,
    shelf: ShelfManager,
    shelf_start: usize,
    shelf_revision: u64,
    shelf_drag: bool,
    shelf_fade_at: Option<Instant>,
    focus: FocusTimer,
    focus_text: String,
    focus_celebrate_at: Option<Instant>,
    water: WaterTracker,
    water_phase: f32,
    water_nudge: bool,
    todos: Vec<TodoItem>,
    todo_editing: bool,
    last_day_check: Instant,
    headphone: HeadphoneWatcher,
    tray: TrayController,
    hud: HudState,
    last_power_poll: Instant,
    power: PowerSnapshot,
    window_visible: bool,
}

#[allow(clippy::too_many_arguments)]
fn install_runtime(
    app: &AppWindow,
    overlay: OverlayWindow,
    media: MediaController,
    audio: Arc<AudioWave>,
    shelf: ShelfManager,
    store: crate::state::StateStore,
    tray: TrayController,
    headphone: HeadphoneWatcher,
) {
    let weak = app.as_weak();
    let (focus_minutes, water_seed, todos) = store.with(|state| {
        (
            state.focus_timer.default_minutes,
            (
                state.water_tracker.current_ml,
                state.water_tracker.daily_goal_ml,
                state.water_tracker.last_drink_timestamp,
            ),
            state.todos.clone(),
        )
    });
    let runtime = Rc::new(RefCell::new(Runtime {
        left_at: None,
        click_through: true,
        last_tick: Instant::now(),
        last_living: Instant::now(),
        last_media_poll: Instant::now() - MEDIA_POLL_INTERVAL,
        ticks: 0,
        media_active: MEDIA_ACTIVE.with(Cell::get),
        media: media.clone(),
        audio,
        playing: false,
        track_generation: 0,
        playback: PlaybackClock {
            position_ms: 0,
            duration_ms: 0,
            playing: false,
            synced_at: Instant::now(),
        },
        elapsed_seconds: u64::MAX,
        lyrics: crate::lyrics::LyricsManager::default(),
        current_lyrics: None,
        last_lyrics_track: String::new(),
        last_synced_line: None,
        store,
        shelf,
        shelf_start: 0,
        shelf_revision: 0,
        shelf_drag: false,
        shelf_fade_at: None,
        focus: FocusTimer::new(focus_minutes),
        focus_text: String::new(),
        focus_celebrate_at: None,
        water: WaterTracker::new(water_seed.0, water_seed.1, water_seed.2),
        water_phase: 0.0,
        water_nudge: false,
        todos,
        todo_editing: false,
        last_day_check: Instant::now(),
        headphone,
        tray,
        hud: HudState::new(),
        last_power_poll: Instant::now(),
        power: PowerSnapshot::from_battery(&crate::system::read_battery()),
        window_visible: true,
    }));

    install_media_callbacks(app, &runtime);
    install_shelf_callbacks(app, &runtime);
    install_productivity_callbacks(app, &runtime, overlay);

    {
        let mut runtime = runtime.borrow_mut();
        sync_focus_ui(app, &mut runtime);
        sync_water_ui(app, &runtime);
        rebuild_todos(app, &runtime);
    }

    match shelf::register_drop_target(overlay.hwnd(), runtime.borrow().shelf.clone()) {
        Ok(()) => crate::log_line("shelf drop target registered"),
        Err(error) => {
            crate::log_line(&format!(
                "failed to register the shelf drop target: {error}"
            ));
            eprintln!("[Kripton] failed to register the shelf drop target: {error}");
        }
    }

    let mut living = Living::new();
    let runtime_for_timer = runtime.clone();
    let weak_for_timer = weak.clone();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, HOVER_POLL_INTERVAL, move || {
        let Some(app) = weak_for_timer.upgrade() else {
            return;
        };

        let now = Instant::now();
        let mut runtime = runtime_for_timer.borrow_mut();
        let dt = (now - runtime.last_tick).as_secs_f32().clamp(0.001, 0.05);
        runtime.last_tick = now;
        runtime.ticks = runtime.ticks.wrapping_add(1);

        // Tray actions are queued from the Win32 message thread; this is the
        // only place that applies them, so Slint is never touched off-thread.
        drain_tray_actions(&app, &mut runtime, overlay);
        if !runtime.window_visible {
            return;
        }

        // Headphone/charging HUDs and the 250 ms power probe.
        update_hardware_hud(&app, &mut runtime, now);

        // Keep music state fresh even while the notch is collapsed: Closed
        // mode depends on it and hover-open must already show the right track.
        if now.duration_since(runtime.last_media_poll) >= MEDIA_POLL_INTERVAL {
            runtime.last_media_poll = now;
            poll_media(&app, &mut runtime, now);
        }

        // Shelf: drain OLE drops, honour an auto-open request, then refresh
        // the Slint model when entries or thumbnails changed.
        let pending = runtime.shelf.take_pending();
        if !pending.is_empty() {
            runtime.shelf.add_paths(pending);
            runtime.shelf_start = runtime.shelf.len().saturating_sub(shelf::VISIBLE_ITEMS);
        }
        if runtime.shelf.take_open_request() {
            open_shelf_page(&app, &mut runtime);
        }
        let drag_active = runtime.shelf.drag_active();
        if runtime.shelf_drag != drag_active {
            runtime.shelf_drag = drag_active;
            app.set_shelf_drag_active(drag_active);
        }
        if runtime.shelf.revision() != runtime.shelf_revision {
            rebuild_shelf(&app, &mut runtime);
        }
        if let Some(fade_at) = runtime.shelf_fade_at {
            if now.duration_since(fade_at) >= Duration::from_millis(30) {
                app.set_shelf_fade(1.0);
                runtime.shelf_fade_at = None;
            }
        }

        // Productivity: Pomodoro countdown, daily rollover, hydration nudge and
        // the animated water surface.
        update_productivity(&app, &mut runtime, overlay, now, dt);

        // Advance the geometry spring every tick for 60 fps morphs.
        MORPH.with(|morph| {
            let mut morph = morph.borrow_mut();
            let config = morph.config;
            let width_moving = morph.width.step(dt, config);
            let height_moving = morph.height.step(dt, config);
            if width_moving || height_moving {
                app.set_notch_width(morph.width.value());
                app.set_notch_height(morph.height.value());
            }

            if let Some(mode) = morph.pending_collapse {
                if morph.width.value() <= COLLAPSE_REVEAL_WIDTH {
                    morph.pending_collapse = None;
                    app.set_expanded(false);
                    app.set_stealth(matches!(mode, NotchMode::Stealth));
                    let show_closed = matches!(mode, NotchMode::Closed | NotchMode::CompactHovered);
                    app.set_closed(show_closed);
                    app.set_hud_visible(false);
                }
            }
        });

        // Hover state and adaptive click-through.
        let inside = overlay.cursor_inside_notch(&app);
        let should_click_through = !inside;
        if runtime.click_through != should_click_through {
            overlay.set_click_through(should_click_through);
            runtime.click_through = should_click_through;
        }

        if runtime.hud.active() {
            // A hardware HUD owns the notch for its 3 second lifetime. Hover
            // must not collapse or expand it mid-animation.
            runtime.left_at = None;
        } else if inside {
            runtime.left_at = None;
            if HOVER_LOCKED_UNTIL_LEAVE.with(Cell::get) {
                // User explicitly clicked to collapse; stay in compact mode until pointer leaves.
            } else if !app.get_expanded() {
                // When cursor enters the domain (e.g. wakes from Stealth or hovers Closed bar),
                // wake up to Compact Pill mode (360x36 hovered).
                // DO NOT auto-expand to 580x192 HUD! Expansion happens only on user click.
                apply_mode(&app, NotchMode::CompactHovered);
            }
        } else {
            HOVER_LOCKED_UNTIL_LEAVE.with(|locked| locked.set(false));

            if app.get_expanded() {
                let left = *runtime.left_at.get_or_insert(now);
                if now.duration_since(left) >= HOVER_CLOSE_DELAY {
                    apply_mode(&app, collapsed_mode());
                    runtime.left_at = None;
                }
            } else if runtime.focus_celebrate_at.is_some() {
                // Keep the compact pill visible for the completion pulse.
                apply_mode(&app, NotchMode::CompactHovered);
                runtime.left_at = None;
            } else {
                apply_mode(&app, collapsed_mode());
                runtime.left_at = None;
            }
        }

        // Timeline: smooth 60 fps interpolation between SMTC polls.
        if app.get_expanded() && app.get_active_page() == 0 {
            update_timeline(&app, &mut runtime, now);
        }

        // Living mascot at ~20 fps, driven by the real spectrum.
        if runtime.ticks.is_multiple_of(LIVING_EVERY) {
            let living_dt = (now - runtime.last_living).as_secs_f32().clamp(0.001, 0.1);
            runtime.last_living = now;

            let frame = living.tick(
                living_dt,
                overlay.cursor_relative_to_notch(&app),
                runtime.audio.peak(),
                runtime.playing,
                runtime.audio.bands(),
            );
            apply_living(&app, &frame);

            if frame.media_active != runtime.media_active {
                runtime.media_active = frame.media_active;
                MEDIA_ACTIVE.with(|active| active.set(frame.media_active));
                if !app.get_expanded() && !runtime.hud.active() {
                    if overlay.cursor_inside_notch(&app) {
                        apply_mode(&app, NotchMode::CompactHovered);
                    } else {
                        apply_mode(&app, collapsed_mode());
                    }
                }
            }
        }
    });

    // The overlay needs its runtime for the whole process lifetime. Leaking
    // this handle is intentional; dropping it would silently stop the loop.
    std::mem::forget(timer);
}

fn drain_tray_actions(app: &AppWindow, runtime: &mut Runtime, overlay: OverlayWindow) {
    for action in runtime.tray.take_actions() {
        match action {
            TrayAction::ToggleVisibility => {
                runtime.window_visible = !runtime.window_visible;
                runtime.hud.dismiss();
                overlay.set_visible(runtime.window_visible);
                runtime.tray.set_visible(runtime.window_visible);
                if runtime.window_visible {
                    apply_mode(app, collapsed_mode());
                }
            }
            TrayAction::SetAutostart(enabled) => {
                if let Err(error) = crate::autostart::set_enabled(enabled) {
                    crate::log_line(&format!("failed to update autostart: {error}"));
                }
            }
            TrayAction::SetLyrics(enabled) => {
                app.set_lyrics_enabled(enabled);
                runtime.store.update(|state| state.lyrics_enabled = enabled);
                runtime.tray.set_lyrics(enabled);
                if app.get_expanded() && app.get_active_page() == 0 {
                    apply_mode(
                        app,
                        if enabled {
                            NotchMode::Lyrics
                        } else {
                            NotchMode::Expanded
                        },
                    );
                }
            }
            TrayAction::SetWaterGoal(goal) => {
                runtime.water.set_goal_ml(goal);
                runtime
                    .store
                    .update(|state| state.water_tracker.daily_goal_ml = goal);
                runtime.tray.set_water_goal(goal);
                sync_water_ui(app, runtime);
            }
            TrayAction::Quit => {
                runtime.tray.shutdown();
                let _ = runtime.store.save();
                let _ = slint::quit_event_loop();
            }
        }
    }
}

fn update_hardware_hud(app: &AppWindow, runtime: &mut Runtime, now: Instant) {
    // If the user clicked or expanded during the HUD lifetime, `apply_mode`
    // already cleared the visible flag. Drop the timer so it cannot override
    // the user's new mode.
    if runtime.hud.active() && !app.get_hud_visible() {
        runtime.hud.dismiss();
    }

    if let Some(mode) = runtime.hud.expire(now) {
        apply_mode(app, mode);
    }

    if now.duration_since(runtime.last_power_poll) >= POWER_POLL_INTERVAL {
        runtime.last_power_poll = now;
        let status = crate::system::read_battery();
        crate::apply_battery(app, status);
        let next = PowerSnapshot::from_battery(&status);
        if let Some(content) = hud::charging_hud_event(runtime.power, next) {
            show_hud(app, runtime, content, now);
        }
        runtime.power = next;
    }

    if let Some(content) = runtime.headphone.take() {
        show_hud(app, runtime, content, now);
    }
}

fn show_hud(app: &AppWindow, runtime: &mut Runtime, content: HudContent, now: Instant) {
    let return_mode = if app.get_expanded() {
        if app.get_active_page() == 0 && app.get_lyrics_enabled() {
            NotchMode::Lyrics
        } else {
            NotchMode::Expanded
        }
    } else if app.get_stealth() {
        NotchMode::Stealth
    } else if app.get_closed() {
        NotchMode::Closed
    } else {
        NotchMode::CompactHovered
    };

    let kind = content.kind;
    apply_hud_ui(app, &content);
    runtime.hud.show(content, now, return_mode);
    apply_mode(
        app,
        match kind {
            HudKind::Headphone => NotchMode::DeviceHud,
            HudKind::Charging => NotchMode::ChargingHud,
        },
    );
}

fn apply_hud_ui(app: &AppWindow, content: &HudContent) {
    app.set_hud_device(matches!(content.kind, HudKind::Headphone));
    app.set_hud_title(content.title.clone().into());
    app.set_hud_status(content.status.clone().into());
    app.set_hud_connected(content.connected);
    app.set_hud_charging(content.charging);
    app.set_hud_percent_text(format!("{}%", content.percent).into());
}

fn install_media_callbacks(app: &AppWindow, runtime: &Rc<RefCell<Runtime>>) {
    let media = runtime.borrow().media.clone();

    let media_play = media.clone();
    app.on_media_play_pause(move || media_play.toggle_play_pause());

    let media_next = media.clone();
    app.on_media_next(move || media_next.next());

    let media_previous = media.clone();
    app.on_media_previous(move || media_previous.previous());

    let runtime_open = runtime.clone();
    app.on_media_open_source(move || {
        let snapshot = runtime_open.borrow().media.snapshot();
        media::open_source(&snapshot.aumid);
    });

    let runtime_preview = runtime.clone();
    let weak_preview = app.as_weak();
    app.on_media_seek_preview(move |fraction| {
        let fraction = fraction.clamp(0.0, 1.0);
        let mut runtime = runtime_preview.borrow_mut();
        let duration = runtime.playback.duration_ms;
        if duration == 0 {
            return;
        }
        let target = (fraction as f64 * duration as f64).round() as u64;
        runtime.elapsed_seconds = target / 1000;
        app_show_position(&weak_preview, fraction, target, duration);
    });

    let runtime_commit = runtime.clone();
    let weak_commit = app.as_weak();
    app.on_media_seek_commit(move |fraction| {
        let fraction = fraction.clamp(0.0, 1.0);
        let mut runtime = runtime_commit.borrow_mut();
        let duration = runtime.playback.duration_ms;
        if duration == 0 {
            return;
        }
        let target = (fraction as f64 * duration as f64).round() as u64;
        runtime.playback.position_ms = target;
        runtime.playback.synced_at = Instant::now();
        runtime.elapsed_seconds = target / 1000;
        runtime.media.seek(target);
        app_show_position(&weak_commit, fraction, target, duration);
    });

    let weak_lyrics = app.as_weak();
    let runtime_lyrics = runtime.clone();
    app.on_lyrics_toggle(move || {
        if let Some(app) = weak_lyrics.upgrade() {
            let new_state = !app.get_lyrics_enabled();
            app.set_page_fade(0.0);
            let weak_fade = app.as_weak();
            Timer::single_shot(Duration::from_millis(20), move || {
                if let Some(app) = weak_fade.upgrade() {
                    app.set_page_fade(1.0);
                }
            });
            app.set_lyrics_enabled(new_state);
            runtime_lyrics
                .borrow()
                .store
                .update(|state| state.lyrics_enabled = new_state);

            if app.get_expanded() && app.get_active_page() == 0 {
                apply_mode(
                    &app,
                    if new_state {
                        NotchMode::Lyrics
                    } else {
                        NotchMode::Expanded
                    },
                );
            }
        }
    });
}

fn install_shelf_callbacks(app: &AppWindow, runtime: &Rc<RefCell<Runtime>>) {
    let runtime_nav = runtime.clone();
    let weak_nav = app.as_weak();
    app.on_shelf_nav(move |delta| {
        let Some(app) = weak_nav.upgrade() else {
            return;
        };
        let mut runtime = runtime_nav.borrow_mut();
        let len = runtime.shelf.len();
        let requested = (runtime.shelf_start as i32 + delta).max(0) as usize;
        runtime.shelf_start = shelf::clamp_start(requested, len);
        rebuild_shelf(&app, &mut runtime);
    });

    let runtime_clear = runtime.clone();
    let weak_clear = app.as_weak();
    app.on_shelf_clear(move || {
        let Some(app) = weak_clear.upgrade() else {
            return;
        };
        let mut runtime = runtime_clear.borrow_mut();
        runtime.shelf.clear();
        runtime.shelf_start = 0;
        rebuild_shelf(&app, &mut runtime);
    });

    let runtime_remove = runtime.clone();
    let weak_remove = app.as_weak();
    app.on_shelf_remove(move |index| {
        let Some(app) = weak_remove.upgrade() else {
            return;
        };
        if index < 0 {
            return;
        }
        let mut runtime = runtime_remove.borrow_mut();
        runtime.shelf.remove(index as usize);
        rebuild_shelf(&app, &mut runtime);
    });

    let runtime_open = runtime.clone();
    app.on_shelf_open(move |index| {
        if index < 0 {
            return;
        }
        let path = runtime_open.borrow().shelf.item_path(index as usize);
        if let Some(path) = path {
            shelf::open_path(&path);
        }
    });
}

fn open_shelf_page(app: &AppWindow, runtime: &mut Runtime) {
    let page = Page::Shelf;
    if app.get_active_page() != page.index() as i32 {
        app.set_page_fade(0.0);
        let weak_fade = app.as_weak();
        Timer::single_shot(Duration::from_millis(20), move || {
            if let Some(app) = weak_fade.upgrade() {
                app.set_page_fade(1.0);
            }
        });
        app.set_active_page(page.index() as i32);
        app.set_page_title(page.title().into());
        app.set_page_subtitle(page.subtitle().into());
        runtime
            .store
            .update(|state| state.active_page = page.index());
    }
    apply_mode(app, NotchMode::Expanded);
}

fn rebuild_shelf(app: &AppWindow, runtime: &mut Runtime) {
    let snapshot = runtime.shelf.snapshot();
    let len = snapshot.items.len();
    runtime.shelf_start = shelf::clamp_start(runtime.shelf_start, len);
    let start = runtime.shelf_start;
    let end = (start + shelf::VISIBLE_ITEMS).min(len);

    let views: Vec<ShelfView> = snapshot.items[start..end]
        .iter()
        .enumerate()
        .map(|(offset, item)| ShelfView {
            name: item.name.clone().into(),
            size_text: if item.missing {
                "Missing".into()
            } else {
                item.size_text.clone().into()
            },
            is_directory: item.is_directory,
            has_thumbnail: snapshot.thumbs.contains_key(&item.path),
            missing: item.missing,
            thumbnail: snapshot
                .thumbs
                .get(&item.path)
                .map(shelf_thumb_image)
                .unwrap_or_default(),
            index: (start + offset) as i32,
        })
        .collect();

    // Restart the fade so new/updated entries ease in instead of popping.
    app.set_shelf_fade(0.0);
    app.set_shelf_items(ModelRc::new(VecModel::from(views)));
    app.set_shelf_start(start as i32);
    app.set_shelf_total(len as i32);
    app.set_shelf_can_prev(start > 0);
    app.set_shelf_can_next(start + shelf::VISIBLE_ITEMS < len);
    app.set_shelf_summary(shelf_summary(len).into());
    runtime.shelf_fade_at = Some(Instant::now());
    runtime.shelf_revision = snapshot.revision;
}

fn shelf_summary(len: usize) -> String {
    match len {
        0 => "Shelf is empty".to_string(),
        1 => "1 item · drop more files".to_string(),
        count => format!("{count} items · drop more files"),
    }
}

fn shelf_thumb_image(thumb: &ShelfThumb) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &thumb.rgba,
        thumb.width,
        thumb.height,
    );
    slint::Image::from_rgba8(buffer)
}

const TODO_ROW_HEIGHT: f32 = 26.0;
const FOCUS_CELEBRATE: Duration = Duration::from_secs(3);

fn install_productivity_callbacks(
    app: &AppWindow,
    runtime: &Rc<RefCell<Runtime>>,
    overlay: OverlayWindow,
) {
    let runtime_toggle = runtime.clone();
    let weak_toggle = app.as_weak();
    app.on_focus_toggle(move || {
        let Some(app) = weak_toggle.upgrade() else {
            return;
        };
        let mut runtime = runtime_toggle.borrow_mut();
        runtime.focus.toggle();
        sync_focus_ui(&app, &mut runtime);
    });

    let runtime_reset = runtime.clone();
    let weak_reset = app.as_weak();
    app.on_focus_reset(move || {
        let Some(app) = weak_reset.upgrade() else {
            return;
        };
        let mut runtime = runtime_reset.borrow_mut();
        runtime.focus.reset();
        sync_focus_ui(&app, &mut runtime);
    });

    let runtime_preset = runtime.clone();
    let weak_preset = app.as_weak();
    app.on_focus_preset(move |minutes| {
        let Some(app) = weak_preset.upgrade() else {
            return;
        };
        let mut runtime = runtime_preset.borrow_mut();
        runtime.focus.set_focus_minutes(minutes.max(0) as u32);
        let minutes = runtime.focus.focus_minutes();
        runtime
            .store
            .update(|state| state.focus_timer.default_minutes = minutes);
        sync_focus_ui(&app, &mut runtime);
    });

    let runtime_adjust = runtime.clone();
    let weak_adjust = app.as_weak();
    app.on_focus_adjust(move |delta| {
        let Some(app) = weak_adjust.upgrade() else {
            return;
        };
        let mut runtime = runtime_adjust.borrow_mut();
        runtime.focus.adjust_focus_minutes(delta);
        let minutes = runtime.focus.focus_minutes();
        runtime
            .store
            .update(|state| state.focus_timer.default_minutes = minutes);
        sync_focus_ui(&app, &mut runtime);
    });

    let runtime_water = runtime.clone();
    let weak_water = app.as_weak();
    app.on_water_add(move |ml| {
        if ml <= 0 {
            return;
        }
        let Some(app) = weak_water.upgrade() else {
            return;
        };
        let mut runtime = runtime_water.borrow_mut();
        let now = unix_now();
        runtime.water.add(ml as u32, now);
        let current = runtime.water.current_ml();
        let last_drink = runtime.water.last_drink();
        let today = today_key();
        runtime.store.update(|state| {
            state.water_tracker.current_ml = current;
            state.water_tracker.last_drink_timestamp = last_drink;
            state.water_tracker.date = today;
        });
        runtime.water_nudge = false;
        app.set_water_nudge(false);
        sync_water_ui(&app, &runtime);
    });

    let runtime_undo = runtime.clone();
    let weak_undo = app.as_weak();
    app.on_water_undo(move || {
        let Some(app) = weak_undo.upgrade() else {
            return;
        };
        let mut runtime = runtime_undo.borrow_mut();
        if runtime.water.undo() {
            let current = runtime.water.current_ml();
            runtime
                .store
                .update(|state| state.water_tracker.current_ml = current);
            sync_water_ui(&app, &runtime);
        }
    });

    let runtime_add = runtime.clone();
    let weak_add = app.as_weak();
    app.on_todo_add(move |text| {
        let Some(app) = weak_add.upgrade() else {
            return;
        };
        let mut runtime = runtime_add.borrow_mut();
        if todo::add(&mut runtime.todos, &text).is_some() {
            persist_todos(&runtime);
            rebuild_todos(&app, &runtime);
        }
    });

    let runtime_toggle_todo = runtime.clone();
    let weak_toggle_todo = app.as_weak();
    app.on_todo_toggle(move |id| {
        if id < 0 {
            return;
        }
        let Some(app) = weak_toggle_todo.upgrade() else {
            return;
        };
        let mut runtime = runtime_toggle_todo.borrow_mut();
        if todo::toggle(&mut runtime.todos, id as u64) {
            persist_todos(&runtime);
            rebuild_todos(&app, &runtime);
        }
    });

    let runtime_remove_todo = runtime.clone();
    let weak_remove_todo = app.as_weak();
    app.on_todo_remove(move |id| {
        if id < 0 {
            return;
        }
        let Some(app) = weak_remove_todo.upgrade() else {
            return;
        };
        let mut runtime = runtime_remove_todo.borrow_mut();
        if todo::remove(&mut runtime.todos, id as u64) {
            persist_todos(&runtime);
            rebuild_todos(&app, &runtime);
        }
    });

    let runtime_clear_todo = runtime.clone();
    let weak_clear_todo = app.as_weak();
    app.on_todo_clear_done(move || {
        let Some(app) = weak_clear_todo.upgrade() else {
            return;
        };
        let mut runtime = runtime_clear_todo.borrow_mut();
        if todo::clear_completed(&mut runtime.todos) {
            persist_todos(&runtime);
            rebuild_todos(&app, &runtime);
        }
    });

    let runtime_reorder = runtime.clone();
    let weak_reorder = app.as_weak();
    app.on_todo_reorder(move |index, dy| {
        if index < 0 {
            return;
        }
        let Some(app) = weak_reorder.upgrade() else {
            return;
        };
        let mut runtime = runtime_reorder.borrow_mut();
        let from = index as usize;
        let target = index + (dy / TODO_ROW_HEIGHT).round() as i32;
        if target < 0 || target as usize >= runtime.todos.len() {
            return;
        }
        if todo::move_item(&mut runtime.todos, from, target as usize) {
            persist_todos(&runtime);
            rebuild_todos(&app, &runtime);
        }
    });

    let runtime_edit = runtime.clone();
    let weak_edit = app.as_weak();
    app.on_todo_edit(move |active| {
        overlay.set_activatable(active);
        runtime_edit.borrow_mut().todo_editing = active;
        if let Some(app) = weak_edit.upgrade() {
            app.set_todo_edit_active(active);
        }
    });
}
fn update_productivity(
    app: &AppWindow,
    runtime: &mut Runtime,
    overlay: OverlayWindow,
    now: Instant,
    dt: f32,
) {
    if runtime.focus.tick().is_some() {
        finish_focus_phase(app, runtime);
    }

    let text = runtime.focus.remaining_text();
    if text != runtime.focus_text {
        runtime.focus_text = text;
        app.set_focus_remaining(runtime.focus_text.clone().into());
        app.set_focus_progress(runtime.focus.progress());
        app.set_focus_running(runtime.focus.running());
        app.set_focus_active(runtime.focus.active());
    }

    // Midnight rollover for the focus and water counters.
    if now.duration_since(runtime.last_day_check) >= Duration::from_secs(60) {
        runtime.last_day_check = now;
        if runtime.store.roll_over_day() {
            let (water, completed) = runtime.store.with(|state| {
                (
                    WaterTracker::new(
                        state.water_tracker.current_ml,
                        state.water_tracker.daily_goal_ml,
                        state.water_tracker.last_drink_timestamp,
                    ),
                    state.focus_timer.completed_sessions_today,
                )
            });
            runtime.water = water;
            runtime.water_nudge = false;
            app.set_water_nudge(false);
            app.set_focus_completed(completed as i32);
            sync_focus_ui(app, runtime);
            sync_water_ui(app, runtime);
        }
    }

    let nudge = runtime
        .water
        .needs_nudge(unix_now(), water::NUDGE_AFTER_SECONDS);
    if nudge != runtime.water_nudge {
        runtime.water_nudge = nudge;
        app.set_water_nudge(nudge);
    }

    if app.get_expanded() && app.get_active_page() == 2 {
        runtime.water_phase = (runtime.water_phase + dt * 0.22) % 1.0;
        app.set_water_phase(runtime.water_phase);
    }

    // Three second plasma pulse when a phase completes.
    if let Some(at) = runtime.focus_celebrate_at {
        let elapsed = now.duration_since(at);
        if elapsed >= FOCUS_CELEBRATE {
            runtime.focus_celebrate_at = None;
            app.set_focus_celebrate(false);
        } else {
            let pulse = 0.5 + 0.5 * (elapsed.as_secs_f32() * 7.0).sin();
            app.set_orb_glow(0.35 + 0.5 * pulse);
            app.set_aura_opacity(0.3 + 0.5 * pulse);
        }
    }

    // Typing needs an activatable window; restore the overlay behaviour as soon
    // as the To-do page or the expanded shell goes away.
    if runtime.todo_editing && !(app.get_expanded() && app.get_active_page() == 4) {
        runtime.todo_editing = false;
        overlay.set_activatable(false);
        app.set_todo_edit_active(false);
    }
}

fn finish_focus_phase(app: &AppWindow, runtime: &mut Runtime) {
    let completed = runtime.focus.complete();
    focus::play_chime();
    runtime.focus_celebrate_at = Some(Instant::now());
    app.set_focus_celebrate(true);

    if completed == FocusPhase::Focus {
        let minutes = runtime.focus.focus_minutes();
        let today = today_key();
        let count = runtime.store.update(|state| {
            if state.focus_timer.last_session_date != today {
                state.focus_timer.last_session_date = today;
                state.focus_timer.completed_sessions_today = 0;
            }
            state.focus_timer.completed_sessions_today =
                state.focus_timer.completed_sessions_today.saturating_add(1);
            state.focus_timer.default_minutes = minutes;
            state.focus_timer.completed_sessions_today
        });
        app.set_focus_completed(count as i32);
    }

    sync_focus_ui(app, runtime);
}

fn sync_focus_ui(app: &AppWindow, runtime: &mut Runtime) {
    let text = runtime.focus.remaining_text();
    runtime.focus_text = text.clone();
    app.set_focus_remaining(text.into());
    app.set_focus_progress(runtime.focus.progress());
    app.set_focus_running(runtime.focus.running());
    app.set_focus_active(runtime.focus.active());
    app.set_focus_minutes(runtime.focus.focus_minutes() as i32);

    let completed = runtime
        .store
        .with(|state| state.focus_timer.completed_sessions_today);
    app.set_focus_completed(completed as i32);
    let session = if runtime.focus.phase() == FocusPhase::Focus {
        format!("Focus Session #{}", completed + 1)
    } else {
        "Short Break".to_string()
    };
    app.set_focus_session(session.into());
}

fn sync_water_ui(app: &AppWindow, runtime: &Runtime) {
    app.set_water_current(runtime.water.current_ml() as i32);
    app.set_water_goal(runtime.water.goal_ml() as i32);
    app.set_water_progress(runtime.water.progress());
    app.set_water_summary(runtime.water.summary_text().into());
    app.set_water_percent(runtime.water.percent_text().into());
    app.set_water_can_undo(runtime.water.undo_available());
}

fn rebuild_todos(app: &AppWindow, runtime: &Runtime) {
    let views: Vec<TodoView> = runtime
        .todos
        .iter()
        .map(|todo| TodoView {
            id: todo.id as i32,
            text: todo.text.clone().into(),
            completed: todo.completed,
        })
        .collect();
    app.set_todo_items(ModelRc::new(VecModel::from(views)));
    app.set_todo_total(runtime.todos.len() as i32);
    app.set_todo_done(todo::completed_count(&runtime.todos) as i32);
    app.set_todo_summary(todo::summary(&runtime.todos).into());
}

fn persist_todos(runtime: &Runtime) {
    runtime
        .store
        .update(|state| state.todos = runtime.todos.clone());
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn today_key() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn app_show_position(weak: &Weak<AppWindow>, fraction: f32, position: u64, duration: u64) {
    let Some(app) = weak.upgrade() else {
        return;
    };
    app.set_seek_fraction(fraction);
    app.set_elapsed_text(format_time(position).into());
    app.set_remaining_text(format!("-{}", format_time(duration.saturating_sub(position))).into());
}

fn poll_media(app: &AppWindow, runtime: &mut Runtime, now: Instant) {
    let snapshot = runtime.media.snapshot();

    if snapshot.generation != runtime.track_generation {
        runtime.track_generation = snapshot.generation;
        app.set_has_album_art(false);
        app.set_lyrics_glow_color(slint::Color::from_argb_u8(255, 30, 58, 138));
    }

    // Artwork is decoded on the SMTC worker; the texture must be built here on
    // the UI thread.
    if !app.get_has_album_art() {
        if let Some(art) = runtime.media.artwork() {
            if art.generation == snapshot.generation {
                app.set_album_art(artwork_image(&art));
                app.set_has_album_art(true);
                let (r, g, b) = art.dominant_color;
                app.set_lyrics_glow_color(slint::Color::from_argb_u8(255, r, g, b));
            }
        }
    }

    let title = if snapshot.title.is_empty() {
        "Nothing playing".to_string()
    } else {
        snapshot.title.clone()
    };
    let subtitle = if snapshot.available {
        snapshot.subtitle()
    } else {
        "Start a track in any media app".to_string()
    };

    app.set_media_available(snapshot.available);
    app.set_media_playing(snapshot.playing);
    app.set_can_skip_previous(snapshot.can_skip_previous);
    app.set_can_skip_next(snapshot.can_skip_next);
    app.set_seek_enabled(snapshot.duration_ms > 0 && snapshot.can_seek);
    if app.get_track_title().as_str() != title.as_str() {
        app.set_track_title(title.into());
    }
    if app.get_track_subtitle().as_str() != subtitle.as_str() {
        app.set_track_subtitle(subtitle.into());
    }

    let track_changed = snapshot.generation != runtime.track_generation;
    let playing_changed = snapshot.playing != runtime.playback.playing;
    let current_projected = runtime.playback.position(now);
    let drift_ms = (snapshot.position_ms as i64 - current_projected as i64).abs();

    // Resync clock only on track change, play/pause change, initial start,
    // or when position drifted significantly (e.g. user seeked in player).
    // During smooth playback, keep local monotonic clock running continuously
    // without resetting synced_at on every poll.
    if track_changed || playing_changed || !runtime.playback.playing || drift_ms > 1500 {
        runtime.playback = PlaybackClock {
            position_ms: snapshot.position_ms,
            duration_ms: snapshot.duration_ms,
            playing: snapshot.playing,
            synced_at: now,
        };
        runtime.elapsed_seconds = u64::MAX;
    } else {
        runtime.playback.duration_ms = snapshot.duration_ms;
    }
    runtime.playing = snapshot.playing;
}

fn update_timeline(app: &AppWindow, runtime: &mut Runtime, now: Instant) {
    if app.get_seek_dragging() {
        return;
    }

    let position = runtime.playback.position(now);
    let duration = runtime.playback.duration_ms;
    if duration > 0 {
        app.set_seek_fraction((position as f64 / duration as f64) as f32);
    }

    let seconds = position / 1000;
    if seconds != runtime.elapsed_seconds {
        runtime.elapsed_seconds = seconds;
        app.set_elapsed_text(format_time(position).into());
        app.set_remaining_text(
            format!("-{}", format_time(duration.saturating_sub(position))).into(),
        );
    }

    // Live lyrics synchronization
    update_lyrics_sync(app, runtime, position);
}

fn update_lyrics_sync(app: &AppWindow, runtime: &mut Runtime, position_ms: u64) {
    let snapshot = runtime.media.snapshot();
    if !snapshot.available || snapshot.title.is_empty() {
        if runtime.last_synced_line.is_some() {
            runtime.last_synced_line = None;
            app.set_lyrics_available(false);
            app.set_lyrics_loading(false);
            app.set_lyrics_line_curr("Play a track to see lyrics".into());
            app.set_lyrics_line_prev("".into());
            app.set_lyrics_line_next("".into());
        }
        return;
    }

    let track_key = crate::lyrics::LyricsManager::track_key(&snapshot.artist, &snapshot.title);
    if runtime.last_lyrics_track != track_key {
        runtime.last_lyrics_track = track_key;
        runtime.current_lyrics =
            runtime
                .lyrics
                .get_or_fetch(&snapshot.artist, &snapshot.title, snapshot.duration_ms);
        runtime.last_synced_line = None;
    } else if runtime.current_lyrics.is_none() {
        runtime.current_lyrics =
            runtime
                .lyrics
                .get_or_fetch(&snapshot.artist, &snapshot.title, snapshot.duration_ms);
    }

    match &runtime.current_lyrics {
        Some(lyrics) if lyrics.is_synced => {
            let (prev, curr, next) = lyrics.current_window(position_ms);
            let curr_str = curr.unwrap_or("♪ ♪ ♪");
            let line_signature =
                format!("{}:{}:{}", prev.unwrap_or(""), curr_str, next.unwrap_or(""));
            if runtime.last_synced_line.as_deref() != Some(&line_signature) {
                runtime.last_synced_line = Some(line_signature);
                app.set_lyrics_available(true);
                app.set_lyrics_loading(false);
                app.set_lyrics_line_prev(prev.unwrap_or("").into());
                app.set_lyrics_line_curr(curr_str.into());
                app.set_lyrics_line_next(next.unwrap_or("").into());
            }
        }
        Some(lyrics) => {
            if runtime.last_synced_line.as_deref() != Some("unsynced") {
                runtime.last_synced_line = Some("unsynced".into());
                app.set_lyrics_available(false);
                app.set_lyrics_loading(false);
                app.set_lyrics_line_curr(
                    lyrics
                        .plain_lyrics
                        .as_deref()
                        .unwrap_or("Lyrics unavailable")
                        .into(),
                );
                app.set_lyrics_line_prev("".into());
                app.set_lyrics_line_next("".into());
            }
        }
        None => {
            if runtime.last_synced_line.as_deref() != Some("loading") {
                runtime.last_synced_line = Some("loading".into());
                app.set_lyrics_available(false);
                app.set_lyrics_loading(true);
                app.set_lyrics_line_curr("Looking for lyrics...".into());
                app.set_lyrics_line_prev("".into());
                app.set_lyrics_line_next("".into());
            }
        }
    }
}

fn artwork_image(art: &Artwork) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &art.rgba, art.width, art.height,
    );
    slint::Image::from_rgba8(buffer)
}

fn format_time(milliseconds: u64) -> String {
    let total = milliseconds / 1000;
    let hours = total / 3600;
    let minutes = (total / 60) % 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn apply_living(app: &AppWindow, frame: &LivingFrame) {
    app.set_orb_scale(frame.orb_scale);
    app.set_orb_glow(frame.orb_glow);
    app.set_aura_scale(frame.aura_scale);
    app.set_aura_opacity(frame.aura_opacity);
    app.set_corona_angle(frame.corona_angle);
    app.set_orb_offset_x(frame.orb_offset_x);
    app.set_orb_offset_y(frame.orb_offset_y);
    app.set_wave_0(frame.wave[0]);
    app.set_wave_1(frame.wave[1]);
    app.set_wave_2(frame.wave[2]);
    app.set_wave_3(frame.wave[3]);
    app.set_wave_4(frame.wave[4]);
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
