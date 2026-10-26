use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    rc::Rc,
    sync::Arc,
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

use crate::{
    living::{Living, LivingFrame},
    media::{self, Artwork, MediaController},
    spring::{Spring, SpringConfig},
    state::NotchMode,
    wave::AudioWave,
    AppWindow,
};

const HOVER_OPEN_DELAY: Duration = Duration::from_millis(60);
const HOVER_CLOSE_DELAY: Duration = Duration::from_millis(250);
const HOVER_POLL_INTERVAL: Duration = Duration::from_millis(16);
const HOVER_PADDING: f32 = 6.0;
const LIVING_EVERY: u64 = 3; // Living properties update at ~20 fps.
const MEDIA_POLL_INTERVAL: Duration = Duration::from_millis(250);
const COLLAPSE_REVEAL_WIDTH: f32 = 250.0;

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
        let (width, height) = NotchMode::Idle.size();
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
    let expanded = matches!(
        mode,
        NotchMode::Expanded | NotchMode::Lyrics | NotchMode::Hud
    );

    MORPH.with(|morph| {
        let mut morph = morph.borrow_mut();
        morph.width.set_target(width);
        morph.height.set_target(height);

        if expanded {
            morph.pending_collapse = None;
            app.set_expanded(true);
            app.set_closed(false);
        } else if app.get_expanded() && morph.width.value() > COLLAPSE_REVEAL_WIDTH {
            // Keep the expanded shell visible until the black shape has shrunk
            // close to compact size, then swap content in the runtime tick.
            morph.pending_collapse = Some(mode);
        } else {
            morph.pending_collapse = None;
            app.set_expanded(false);
            app.set_closed(matches!(mode, NotchMode::Closed));
        }
    });
}

/// Mode to return to when the pointer leaves the notch: Closed while media is
/// playing, otherwise Idle.
pub fn collapsed_mode() -> NotchMode {
    if MEDIA_ACTIVE.with(Cell::get) {
        NotchMode::Closed
    } else {
        NotchMode::Idle
    }
}

pub fn install_overlay(
    app: &AppWindow,
    media: MediaController,
    audio: AudioWave,
    store: crate::state::StateStore,
) {
    try_attach(app.as_weak(), 0, media, Arc::new(audio), store);
}

fn try_attach(
    app: Weak<AppWindow>,
    attempts: u32,
    media: MediaController,
    audio: Arc<AudioWave>,
    store: crate::state::StateStore,
) {
    Timer::single_shot(Duration::from_millis(16), move || {
        let Some(app) = app.upgrade() else {
            return;
        };

        if let Some(overlay) = OverlayWindow::attach(&app) {
            install_runtime(&app, overlay, media, audio, store);
        } else if attempts < 120 {
            try_attach(
                app.as_weak(),
                attempts + 1,
                media.clone(),
                audio.clone(),
                store,
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
    entered_at: Option<Instant>,
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
}

fn install_runtime(
    app: &AppWindow,
    overlay: OverlayWindow,
    media: MediaController,
    audio: Arc<AudioWave>,
    store: crate::state::StateStore,
) {
    let weak = app.as_weak();
    let runtime = Rc::new(RefCell::new(Runtime {
        entered_at: None,
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
    }));

    install_media_callbacks(app, &runtime);

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

        // Keep music state fresh even while the notch is collapsed: Closed
        // mode depends on it and hover-open must already show the right track.
        if now.duration_since(runtime.last_media_poll) >= MEDIA_POLL_INTERVAL {
            runtime.last_media_poll = now;
            poll_media(&app, &mut runtime, now);
        }

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
                    app.set_closed(matches!(mode, NotchMode::Closed));
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

        if inside {
            runtime.left_at = None;
            if HOVER_LOCKED_UNTIL_LEAVE.with(Cell::get) {
                runtime.entered_at = None;
            } else if !app.get_expanded() {
                let entered = *runtime.entered_at.get_or_insert(now);
                if now.duration_since(entered) >= HOVER_OPEN_DELAY {
                    let mode = if app.get_active_page() == 0 && app.get_lyrics_enabled() {
                        NotchMode::Lyrics
                    } else {
                        NotchMode::Expanded
                    };
                    apply_mode(&app, mode);
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
                    apply_mode(&app, collapsed_mode());
                    runtime.left_at = None;
                }
            } else {
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
                if !app.get_expanded() {
                    apply_mode(&app, collapsed_mode());
                }
            }
        }
    });

    // The overlay needs its runtime for the whole process lifetime. Leaking
    // this handle is intentional; dropping it would silently stop the loop.
    std::mem::forget(timer);
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

    runtime.playing = snapshot.playing;
    runtime.playback = PlaybackClock {
        position_ms: snapshot.position_ms,
        duration_ms: snapshot.duration_ms,
        playing: snapshot.playing,
        synced_at: now,
    };
    runtime.elapsed_seconds = u64::MAX;
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
            let line_signature = format!("{}:{}", prev.unwrap_or(""), curr_str);
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
