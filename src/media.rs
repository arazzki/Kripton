//! WinRT SMTC integration: metadata, transport, seek and album art.
//!
//! WinRT is asynchronous, but the rest of Kripton is a plain winit/Slint event
//! loop. A dedicated MTA worker owns the session manager and blocks on the
//! `IAsyncOperation`s with a tiny park/unpark executor, then publishes a plain
//! snapshot that the UI thread can read without touching COM.

use std::{
    future::{Future, IntoFuture},
    pin::pin,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    task::{Context, Poll, Wake, Waker},
    thread,
    time::{Duration, Instant},
};

use windows::{
    core::{w, Interface, PCWSTR},
    Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    },
    Storage::Streams::{Buffer, InputStreamOptions},
    Win32::{
        System::{
            Com::{CoCreateInstance, CLSCTX_ALL},
            WinRT::{IBufferByteAccess, RoInitialize, RO_INIT_MULTITHREADED},
        },
        UI::{
            Shell::{
                ApplicationActivationManager, IApplicationActivationManager, ShellExecuteW, AO_NONE,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

const POLL_INTERVAL: Duration = Duration::from_millis(300);
const MANAGER_RETRY: Duration = Duration::from_secs(2);
const ART_RETRY: Duration = Duration::from_secs(3);
const ART_SIZE: u32 = 128;
const MAX_ART_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct MediaSnapshot {
    pub available: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source_app: String,
    pub aumid: String,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub can_seek: bool,
    pub can_skip_previous: bool,
    pub can_skip_next: bool,
    /// Bumped whenever the track identity changes, so the UI knows to rebuild
    /// the album-art texture.
    pub generation: u64,
}

impl MediaSnapshot {
    /// "Artist - Album", skipping empty halves.
    pub fn subtitle(&self) -> String {
        match (self.artist.is_empty(), self.album.is_empty()) {
            (false, false) => format!("{} - {}", self.artist, self.album),
            (false, true) => self.artist.clone(),
            (true, false) => self.album.clone(),
            (true, true) => self.source_app.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Artwork {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
struct Shared {
    snapshot: Mutex<MediaSnapshot>,
    artwork: Mutex<Option<Artwork>>,
}

#[derive(Debug, Clone, Copy)]
enum Command {
    TogglePlayPause,
    Next,
    Previous,
    Seek(u64),
}

/// UI-side handle. Cloning shares the same worker and snapshot.
#[derive(Clone)]
pub struct MediaController {
    shared: Arc<Shared>,
    commands: Sender<Command>,
}

impl MediaController {
    pub fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let (commands, receiver) = mpsc::channel();
        let worker_shared = shared.clone();

        thread::Builder::new()
            .name("kripton-smtc".into())
            .spawn(move || media_loop(worker_shared, receiver))
            .expect("failed to spawn SMTC worker");

        Self { shared, commands }
    }

    pub fn snapshot(&self) -> MediaSnapshot {
        self.shared
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn artwork(&self) -> Option<Artwork> {
        self.shared
            .artwork
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn toggle_play_pause(&self) {
        let _ = self.commands.send(Command::TogglePlayPause);
    }

    pub fn next(&self) {
        let _ = self.commands.send(Command::Next);
    }

    pub fn previous(&self) {
        let _ = self.commands.send(Command::Previous);
    }

    pub fn seek(&self, position_ms: u64) {
        let _ = self.commands.send(Command::Seek(position_ms));
    }
}

/// Focus the app that owns the current media session. Must run on the STA
/// (UI) thread. Falls back to `shell:AppsFolder` when the packaged-app
/// activation path is unavailable (classic Win32 players such as Spotify).
pub fn open_source(aumid: &str) {
    if aumid.is_empty() {
        return;
    }

    let wide: Vec<u16> = aumid.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let manager: windows::core::Result<IApplicationActivationManager> =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL);
        if let Ok(manager) = manager {
            if manager
                .ActivateApplication(PCWSTR(wide.as_ptr()), w!(""), AO_NONE)
                .is_ok()
            {
                return;
            }
        }

        let folder: Vec<u16> = format!("shell:AppsFolder\\{aumid}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        ShellExecuteW(
            None,
            w!("open"),
            w!("explorer.exe"),
            PCWSTR(folder.as_ptr()),
            None,
            SW_SHOWNORMAL,
        );
    }
}

fn media_loop(shared: Arc<Shared>, commands: Receiver<Command>) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    let mut manager: Option<GlobalSystemMediaTransportControlsSessionManager> = None;
    let mut last_manager_attempt = Instant::now() - MANAGER_RETRY;
    let mut current: Option<GlobalSystemMediaTransportControlsSession> = None;
    let mut last_poll = Instant::now() - POLL_INTERVAL;

    let mut generation = 0_u64;
    let mut track_key = String::new();
    let mut last_art_attempt = Instant::now() - ART_RETRY;
    let mut art_generation = 0_u64;
    let mut logged_manager_failure = false;

    loop {
        let wait = POLL_INTERVAL.saturating_sub(last_poll.elapsed());
        match commands.recv_timeout(wait) {
            Ok(command) => {
                run_command(current.as_ref(), command);
                // Refresh soon so the new state reaches the UI quickly.
                last_poll = Instant::now() - POLL_INTERVAL;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        if manager.is_none() && last_manager_attempt.elapsed() >= MANAGER_RETRY {
            last_manager_attempt = Instant::now();
            match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
                Ok(operation) => match block_on(operation.into_future()) {
                    Ok(found) => {
                        manager = Some(found);
                        logged_manager_failure = false;
                    }
                    Err(error) => {
                        if !logged_manager_failure {
                            crate::log_line(&format!("SMTC manager request failed: {error}"));
                            logged_manager_failure = true;
                        }
                    }
                },
                Err(error) => {
                    if !logged_manager_failure {
                        crate::log_line(&format!("SMTC unavailable: {error}"));
                        logged_manager_failure = true;
                    }
                }
            }
        }

        let Some(manager) = manager.as_ref() else {
            continue;
        };
        if last_poll.elapsed() < POLL_INTERVAL {
            continue;
        }
        last_poll = Instant::now();

        current = manager.GetCurrentSession().ok();
        let Some(session) = current.as_ref() else {
            publish_unavailable(&shared);
            continue;
        };

        let source_app = session
            .SourceAppUserModelId()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let playback = session.GetPlaybackInfo().ok();
        let playing = playback
            .as_ref()
            .and_then(|info| info.PlaybackStatus().ok())
            .is_some_and(|status| {
                status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
            });
        let controls = playback.as_ref().and_then(|info| info.Controls().ok());
        let can_seek = controls
            .as_ref()
            .and_then(|controls| controls.IsPlaybackPositionEnabled().ok())
            .unwrap_or(false);
        let can_skip_previous = controls
            .as_ref()
            .and_then(|controls| controls.IsPreviousEnabled().ok())
            .unwrap_or(false);
        let can_skip_next = controls
            .as_ref()
            .and_then(|controls| controls.IsNextEnabled().ok())
            .unwrap_or(false);

        let properties = session
            .TryGetMediaPropertiesAsync()
            .ok()
            .and_then(|operation| block_on(operation.into_future()).ok());
        let (title, artist, album, thumbnail) = match properties {
            Some(properties) => (
                properties
                    .Title()
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                properties
                    .Artist()
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                properties
                    .AlbumTitle()
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                properties.Thumbnail().ok(),
            ),
            None => (String::new(), String::new(), String::new(), None),
        };

        let (position_ms, duration_ms) = session
            .GetTimelineProperties()
            .map(|timeline| {
                let start = timeline.StartTime().map(|span| span.Duration).unwrap_or(0);
                let end = timeline.EndTime().map(|span| span.Duration).unwrap_or(0);
                let position = timeline.Position().map(|span| span.Duration).unwrap_or(0);
                (
                    ticks_to_ms((position - start).max(0)),
                    ticks_to_ms((end - start).max(0)),
                )
            })
            .unwrap_or((0, 0));

        let key = format!("{source_app}\u{1}{title}\u{1}{artist}");
        if key != track_key {
            track_key = key;
            generation = generation.wrapping_add(1);
            art_generation = 0;
            *shared
                .artwork
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            last_art_attempt = Instant::now() - ART_RETRY;
        }

        if generation != art_generation && last_art_attempt.elapsed() >= ART_RETRY {
            last_art_attempt = Instant::now();
            if let Some(reference) = thumbnail {
                if let Some(bytes) = read_thumbnail(&reference) {
                    if let Some(artwork) = decode_artwork(&bytes, generation) {
                        art_generation = generation;
                        *shared
                            .artwork
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(artwork);
                    }
                }
            }
        }

        let available = !title.is_empty() || playing;
        let snapshot = MediaSnapshot {
            available,
            playing,
            title,
            artist,
            album,
            source_app: app_label(&source_app),
            aumid: source_app,
            position_ms: position_ms.min(if duration_ms > 0 {
                duration_ms
            } else {
                u64::MAX
            }),
            duration_ms,
            can_seek,
            can_skip_previous,
            can_skip_next,
            generation,
        };
        *shared
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
    }
}

fn publish_unavailable(shared: &Shared) {
    let mut snapshot = shared
        .snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !snapshot.available && !snapshot.playing {
        return;
    }
    snapshot.available = false;
    snapshot.playing = false;
    snapshot.title.clear();
    snapshot.artist.clear();
    snapshot.album.clear();
    snapshot.position_ms = 0;
    snapshot.duration_ms = 0;
    snapshot.can_seek = false;
    snapshot.generation = snapshot.generation.wrapping_add(1);
    shared
        .artwork
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

fn run_command(session: Option<&GlobalSystemMediaTransportControlsSession>, command: Command) {
    let Some(session) = session else {
        return;
    };
    match command {
        Command::TogglePlayPause => {
            if let Ok(operation) = session.TryTogglePlayPauseAsync() {
                let _ = block_on(operation.into_future());
            }
        }
        Command::Next => {
            if let Ok(operation) = session.TrySkipNextAsync() {
                let _ = block_on(operation.into_future());
            }
        }
        Command::Previous => {
            if let Ok(operation) = session.TrySkipPreviousAsync() {
                let _ = block_on(operation.into_future());
            }
        }
        Command::Seek(position_ms) => {
            let ticks = (position_ms as i64).saturating_mul(10_000);
            if let Ok(operation) = session.TryChangePlaybackPositionAsync(ticks) {
                let _ = block_on(operation.into_future());
            }
        }
    }
}

fn ticks_to_ms(ticks: i64) -> u64 {
    (ticks / 10_000).max(0) as u64
}

fn read_thumbnail(
    reference: &windows::Storage::Streams::IRandomAccessStreamReference,
) -> Option<Vec<u8>> {
    let operation = reference.OpenReadAsync().ok()?;
    let stream = block_on(operation.into_future()).ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_ART_BYTES {
        return None;
    }

    let buffer = Buffer::Create(size as u32).ok()?;
    let input = stream.GetInputStreamAt(0).ok()?;
    let read = input
        .ReadAsync(&buffer, size as u32, InputStreamOptions::None)
        .ok()?;
    block_on(read.into_future()).ok()?;

    let length = buffer.Length().ok()?.min(size as u32) as usize;
    let access: IBufferByteAccess = buffer.cast().ok()?;
    let data = unsafe { access.Buffer().ok()? };
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, length) }.to_vec())
}

fn decode_artwork(bytes: &[u8], generation: u64) -> Option<Artwork> {
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image
        .resize_to_fill(ART_SIZE, ART_SIZE, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(Artwork {
        generation,
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

fn app_label(aumid: &str) -> String {
    let lower = aumid.to_ascii_lowercase();
    for (needle, label) in [
        ("spotify", "Spotify"),
        ("applemusic", "Apple Music"),
        ("itunes", "Apple Music"),
        ("msedge", "Edge"),
        ("chrome", "Chrome"),
        ("firefox", "Firefox"),
        ("wmplayer", "Media Player"),
        ("mediaplayer", "Media Player"),
        ("zune", "Groove"),
    ] {
        if lower.contains(needle) {
            return label.to_string();
        }
    }
    aumid
        .split('!')
        .next()
        .unwrap_or(aumid)
        .trim_end_matches(".exe")
        .to_string()
}

/// Minimal blocking executor for `windows-future` operations. The completion
/// handler installed by `AsyncFuture` calls `wake`, which unparks this thread.
fn block_on<F: Future>(future: F) -> F::Output {
    struct ThreadWake(thread::Thread);

    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_skips_empty_parts() {
        let snapshot = MediaSnapshot {
            artist: "Lumen Coast".into(),
            album: "Summer Flows".into(),
            ..Default::default()
        };
        assert_eq!(snapshot.subtitle(), "Lumen Coast - Summer Flows");

        let artist_only = MediaSnapshot {
            artist: "Lumen Coast".into(),
            ..Default::default()
        };
        assert_eq!(artist_only.subtitle(), "Lumen Coast");
    }

    #[test]
    fn ticks_convert_to_milliseconds() {
        assert_eq!(ticks_to_ms(10_000), 1);
        assert_eq!(ticks_to_ms(3_130_000), 313);
        assert_eq!(ticks_to_ms(-10_000), 0);
    }

    #[test]
    fn app_labels_are_friendly() {
        assert_eq!(app_label("Spotify.exe"), "Spotify");
        assert_eq!(app_label("MSEdge"), "Edge");
        assert_eq!(
            app_label("AppleInc.AppleMusicWin_nzyj5cx40ttqa!App"),
            "Apple Music"
        );
        assert_eq!(app_label("SomePlayer.exe"), "SomePlayer");
    }

    #[test]
    fn block_on_resolves_ready_future() {
        assert_eq!(block_on(async { 42 }), 42);
    }
}
