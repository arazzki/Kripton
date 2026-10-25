use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::DataReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    #[allow(dead_code)]
    Play,
    #[allow(dead_code)]
    Pause,
    TogglePlayPause,
    Next,
    Previous,
}

#[derive(Debug)]
enum WorkerEvent {
    WinRtEvent,
    Command(MediaCommand),
    Tick,
}

#[derive(Clone)]
pub struct MediaControllerHandle {
    command_tx: Sender<MediaCommand>,
}

impl MediaControllerHandle {
    pub fn send(&self, cmd: MediaCommand) {
        let _ = self.command_tx.send(cmd);
    }
}

pub struct MediaController;

impl MediaController {
    pub fn start(weak_app: slint::Weak<crate::AppWindow>) -> MediaControllerHandle {
        let (command_tx, command_rx) = channel::<MediaCommand>();
        let (worker_tx, worker_rx) = channel::<WorkerEvent>();

        // Bridge command_rx into worker_tx
        let worker_tx_bridge = worker_tx.clone();
        std::thread::Builder::new()
            .name("kripton-media-bridge".into())
            .spawn(move || {
                while let Ok(cmd) = command_rx.recv() {
                    if worker_tx_bridge.send(WorkerEvent::Command(cmd)).is_err() {
                        break;
                    }
                }
            })
            .expect("Failed to spawn media bridge thread");

        // Main media worker thread
        std::thread::Builder::new()
            .name("kripton-media-worker".into())
            .spawn(move || {
                run_media_worker(worker_tx, worker_rx, weak_app);
            })
            .expect("Failed to spawn media worker thread");

        MediaControllerHandle { command_tx }
    }
}

fn run_media_worker(
    worker_tx: Sender<WorkerEvent>,
    worker_rx: Receiver<WorkerEvent>,
    weak_app: slint::Weak<crate::AppWindow>,
) {
    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
        Ok(op) => match op.join() {
            Ok(mgr) => mgr,
            Err(e) => {
                eprintln!("[Kripton] Failed to initialize SMTC Session Manager: {:?}", e);
                return;
            }
        },
        Err(e) => {
            eprintln!("[Kripton] Failed to request SMTC Session Manager: {:?}", e);
            return;
        }
    };

    // Subscribe to session change events
    let tx_sess = worker_tx.clone();
    let _ = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_mgr, _args| {
        let _ = tx_sess.send(WorkerEvent::WinRtEvent);
        Ok(())
    }));

    let mut current_session: Option<GlobalSystemMediaTransportControlsSession> = None;
    let mut last_thumb_bytes: Option<Vec<u8>> = None;
    let mut is_playing = false;

    // Initial session setup
    refresh_session(
        &manager,
        &mut current_session,
        &worker_tx,
        &mut last_thumb_bytes,
        &mut is_playing,
        &weak_app,
    );

    loop {
        // Dynamic wait duration: 1000ms if playing (for timeline progress), 4000ms if idle/paused
        let timeout = if is_playing {
            Duration::from_millis(1000)
        } else {
            Duration::from_millis(4000)
        };

        let event = match worker_rx.recv_timeout(timeout) {
            Ok(ev) => ev,
            Err(RecvTimeoutError::Timeout) => WorkerEvent::Tick,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        match event {
            WorkerEvent::Command(cmd) => {
                if let Some(session) = &current_session {
                    execute_command(session, cmd);
                } else if let Ok(session) = manager.GetCurrentSession() {
                    execute_command(&session, cmd);
                }

                // Small delay to let OS update media status
                std::thread::sleep(Duration::from_millis(50));

                refresh_session(
                    &manager,
                    &mut current_session,
                    &worker_tx,
                    &mut last_thumb_bytes,
                    &mut is_playing,
                    &weak_app,
                );
            }
            WorkerEvent::WinRtEvent | WorkerEvent::Tick => {
                refresh_session(
                    &manager,
                    &mut current_session,
                    &worker_tx,
                    &mut last_thumb_bytes,
                    &mut is_playing,
                    &weak_app,
                );
            }
        }
    }
}

fn execute_command(session: &GlobalSystemMediaTransportControlsSession, cmd: MediaCommand) {
    let res = match cmd {
        MediaCommand::Play => session.TryPlayAsync().and_then(|op| op.join()),
        MediaCommand::Pause => session.TryPauseAsync().and_then(|op| op.join()),
        MediaCommand::TogglePlayPause => session.TryTogglePlayPauseAsync().and_then(|op| op.join()),
        MediaCommand::Next => session.TrySkipNextAsync().and_then(|op| op.join()),
        MediaCommand::Previous => session.TrySkipPreviousAsync().and_then(|op| op.join()),
    };
    if let Err(e) = res {
        eprintln!("[Kripton] Media command {:?} failed: {:?}", cmd, e);
    }
}

fn refresh_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
    current_session: &mut Option<GlobalSystemMediaTransportControlsSession>,
    worker_tx: &Sender<WorkerEvent>,
    last_thumb_bytes: &mut Option<Vec<u8>>,
    is_playing_state: &mut bool,
    weak_app: &slint::Weak<crate::AppWindow>,
) {
    let session_res = manager.GetCurrentSession();

    match session_res {
        Ok(new_session) => {
            // Subscribe to session events
            let tx1 = worker_tx.clone();
            let _ = new_session.MediaPropertiesChanged(&TypedEventHandler::new(move |_sess, _args| {
                let _ = tx1.send(WorkerEvent::WinRtEvent);
                Ok(())
            }));

            let tx2 = worker_tx.clone();
            let _ = new_session.PlaybackInfoChanged(&TypedEventHandler::new(move |_sess, _args| {
                let _ = tx2.send(WorkerEvent::WinRtEvent);
                Ok(())
            }));

            let tx3 = worker_tx.clone();
            let _ = new_session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_sess, _args| {
                let _ = tx3.send(WorkerEvent::WinRtEvent);
                Ok(())
            }));

            let app_id = new_session
                .SourceAppUserModelId()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let source_title = format_source_name(&app_id);

            // Read playback info
            let playing = new_session
                .GetPlaybackInfo()
                .ok()
                .and_then(|info| info.PlaybackStatus().ok())
                .map(|status| status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
                .unwrap_or(false);

            *is_playing_state = playing;

            // Read media properties
            let (title, artist, thumb_bytes) = if let Ok(props) = new_session.TryGetMediaPropertiesAsync().and_then(|op| op.join()) {
                let t = props.Title().map(|s| s.to_string()).unwrap_or_default();
                let a = props.Artist().map(|s| s.to_string()).unwrap_or_default();

                let mut tb = None;
                if let Ok(thumb_ref) = props.Thumbnail() {
                    if let Ok(stream) = thumb_ref.OpenReadAsync().and_then(|op| op.join()) {
                        if let Ok(size) = stream.Size() {
                            if size > 0 && size < 10 * 1024 * 1024 {
                                if let Ok(reader) = DataReader::CreateDataReader(&stream) {
                                    if reader.LoadAsync(size as u32).and_then(|op| op.join()).is_ok() {
                                        let mut buffer = vec![0u8; size as usize];
                                        if reader.ReadBytes(&mut buffer).is_ok() {
                                            tb = Some(buffer);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                (t, a, tb)
            } else {
                (String::new(), String::new(), None)
            };

            // Read timeline
            let (_pos_secs, _dur_secs, ratio, pos_text, dur_text) = if let Ok(timeline) = new_session.GetTimelineProperties() {
                let pos = timeline.Position().map(|p| p.Duration as f64 / 10_000_000.0).unwrap_or(0.0);
                let dur = timeline.EndTime().map(|p| p.Duration as f64 / 10_000_000.0).unwrap_or(0.0);
                let r = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
                (pos, dur, r, format_time(pos), format_time(dur))
            } else {
                (0.0, 0.0, 0.0, "0:00".into(), "0:00".into())
            };

            // Determine if album art changed
            let mut new_thumb_for_ui: Option<Vec<u8>> = None;
            let mut has_new_img = false;

            if let Some(tb) = &thumb_bytes {
                let is_different = match last_thumb_bytes.as_ref() {
                    Some(old) => old != tb,
                    None => true,
                };
                if is_different {
                    new_thumb_for_ui = Some(tb.clone());
                    has_new_img = true;
                    *last_thumb_bytes = Some(tb.clone());
                } else {
                    has_new_img = true;
                }
            } else {
                *last_thumb_bytes = None;
            }

            *current_session = Some(new_session);

            // Dispatch update to Slint UI
            let _ = weak_app.upgrade_in_event_loop(move |app| {
                app.set_has_media(true);
                app.set_is_playing(playing);
                app.set_track_title(if title.is_empty() { "Unknown Title".into() } else { title.into() });
                app.set_track_artist(artist.into());
                app.set_source_title(source_title.into());
                app.set_progress_ratio(ratio);
                app.set_position_text(pos_text.into());
                app.set_duration_text(dur_text.into());

                if has_new_img {
                    if let Some(bytes) = new_thumb_for_ui {
                        if let Some(img) = bytes_to_slint_image(&bytes) {
                            app.set_album_art(img);
                        }
                    }
                    app.set_has_album_art(true);
                } else if thumb_bytes.is_none() {
                    app.set_has_album_art(false);
                }
            });
        }
        Err(_) => {
            *current_session = None;
            *last_thumb_bytes = None;
            *is_playing_state = false;

            let _ = weak_app.upgrade_in_event_loop(move |app| {
                app.set_has_media(false);
                app.set_is_playing(false);
                app.set_track_title("No Media Playing".into());
                app.set_track_artist("".into());
                app.set_source_title("".into());
                app.set_has_album_art(false);
                app.set_progress_ratio(0.0);
                app.set_position_text("0:00".into());
                app.set_duration_text("0:00".into());
            });
        }
    }
}

fn format_source_name(app_id: &str) -> String {
    let lower = app_id.to_lowercase();
    if lower.contains("spotify") {
        "Spotify".into()
    } else if lower.contains("chrome") {
        "Chrome".into()
    } else if lower.contains("msedge") {
        "Edge".into()
    } else if lower.contains("brave") {
        "Brave".into()
    } else if lower.contains("applemusic") {
        "Apple Music".into()
    } else if lower.contains("vlc") {
        "VLC".into()
    } else if !app_id.is_empty() {
        // Extract basic name
        app_id.split('!').next().unwrap_or(app_id).to_string()
    } else {
        String::new()
    }
}

fn format_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{}:{:02}", m, s)
}

pub fn bytes_to_slint_image(bytes: &[u8]) -> Option<slint::Image> {
    if let Ok(dyn_img) = image::load_from_memory(bytes) {
        let rgba = dyn_img.into_rgba8();
        let (width, height) = rgba.dimensions();
        let pixel_buffer = slint::SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(),
            width,
            height,
        );
        Some(slint::Image::from_rgba8(pixel_buffer))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_time() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(9.0), "0:09");
        assert_eq!(format_time(65.0), "1:05");
        assert_eq!(format_time(175.9), "2:55");
        assert_eq!(format_time(3600.0), "60:00");
    }

    #[test]
    fn test_format_source_name() {
        assert_eq!(format_source_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(format_source_name("Google.Chrome.123"), "Chrome");
        assert_eq!(format_source_name("Microsoft.MSEdge.456"), "Edge");
        assert_eq!(format_source_name("BraveSoftware.Brave"), "Brave");
        assert_eq!(format_source_name("AppleMusic"), "Apple Music");
        assert_eq!(format_source_name("VideoLAN.VLC"), "VLC");
        assert_eq!(format_source_name(""), "");
    }

    #[test]
    fn test_bytes_to_slint_image_invalid() {
        let dummy = vec![0, 1, 2, 3];
        assert!(bytes_to_slint_image(&dummy).is_none());
    }

    #[test]
    fn test_bytes_to_slint_image_valid_png() {
        let img_buf = image::RgbaImage::new(2, 2);
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        img_buf.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let img = bytes_to_slint_image(&bytes);
        assert!(img.is_some());
    }
}
