use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
}

#[derive(Debug, Clone)]
pub struct ShelfItem {
    pub path: PathBuf,
    pub filename: String,
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
    pub height: u32,
    pub resolution_text: String,
    pub thumbnail_rgba: Vec<u8>,
    pub thumb_width: u32,
    pub thumb_height: u32,
}

#[derive(Debug, Clone)]
pub enum ShelfAction {
    OpenLatest,
    CopyPathLatest,
    Dismiss,
}

#[derive(Clone)]
pub struct ShelfHandle {
    action_tx: Sender<ShelfAction>,
}

impl ShelfHandle {
    pub fn send(&self, action: ShelfAction) {
        let _ = self.action_tx.send(action);
    }
}

pub struct ShelfManager;

impl ShelfManager {
    pub fn start(weak_app: slint::Weak<crate::AppWindow>) -> ShelfHandle {
        let (action_tx, action_rx) = channel::<ShelfAction>();
        let latest_item = Arc::new(Mutex::new(None::<ShelfItem>));
        let shelf_timer_counter = Arc::new(AtomicU32::new(0));

        // Auto-dismiss countdown timer thread (runs every 100ms)
        let weak_app_timer = weak_app.clone();
        let counter_timer = shelf_timer_counter.clone();
        std::thread::Builder::new()
            .name("kripton-shelf-timer".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    let val = counter_timer.load(Ordering::Relaxed);
                    if val > 0 {
                        let new_val = val.saturating_sub(1);
                        counter_timer.store(new_val, Ordering::Relaxed);
                        if new_val == 0 {
                            let _ = weak_app_timer.upgrade_in_event_loop(|app| {
                                app.set_is_shelf_active(false);
                            });
                        }
                    }
                }
            })
            .expect("Failed to spawn shelf dismiss timer");

        // Action dispatcher thread
        let latest_for_actions = latest_item.clone();
        let counter_for_actions = shelf_timer_counter.clone();
        let weak_app_actions = weak_app.clone();
        std::thread::Builder::new()
            .name("kripton-shelf-actions".into())
            .spawn(move || {
                while let Ok(action) = action_rx.recv() {
                    match action {
                        ShelfAction::OpenLatest => {
                            let guard = latest_for_actions.lock().unwrap();
                            if let Some(item) = guard.as_ref() {
                                open_file(&item.path);
                            }
                        }
                        ShelfAction::CopyPathLatest => {
                            let guard = latest_for_actions.lock().unwrap();
                            if let Some(item) = guard.as_ref() {
                                if let Some(path_str) = item.path.to_str() {
                                    copy_to_clipboard(path_str);
                                }
                            }
                        }
                        ShelfAction::Dismiss => {
                            counter_for_actions.store(0, Ordering::Relaxed);
                            let _ = weak_app_actions.upgrade_in_event_loop(|app| {
                                app.set_is_shelf_active(false);
                            });
                        }
                    }
                }
            })
            .expect("Failed to spawn shelf actions thread");

        // Clipboard screenshot watcher thread
        let latest_for_watcher = latest_item;
        let counter_for_watcher = shelf_timer_counter;
        std::thread::Builder::new()
            .name("kripton-clipboard-watcher".into())
            .spawn(move || {
                run_clipboard_watcher(weak_app, latest_for_watcher, counter_for_watcher);
            })
            .expect("Failed to spawn clipboard watcher thread");

        ShelfHandle { action_tx }
    }
}

fn run_clipboard_watcher(
    weak_app: slint::Weak<crate::AppWindow>,
    latest_item: Arc<Mutex<Option<ShelfItem>>>,
    shelf_timer_counter: Arc<AtomicU32>,
) {
    let mut last_seq = unsafe { GetClipboardSequenceNumber() };
    let mut last_hash: u64 = 0;
    let shelf_dir = get_or_create_shelf_dir();

    loop {
        std::thread::sleep(Duration::from_millis(350));
        let seq = unsafe { GetClipboardSequenceNumber() };

        if seq != last_seq {
            last_seq = seq;

            // Check if clipboard contains an image
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(img_data) = clipboard.get_image() {
                    let w = img_data.width as u32;
                    let h = img_data.height as u32;

                    // Filter out microscopic or corrupted icons
                    if w >= 24 && h >= 24 {
                        let hash = compute_quick_hash(w, h, &img_data.bytes);
                        if hash != last_hash {
                            last_hash = hash;

                            if let Some(item) = process_new_screenshot(
                                &shelf_dir,
                                w,
                                h,
                                img_data.bytes.into_owned(),
                            ) {
                                // Store in latest
                                {
                                    let mut guard = latest_item.lock().unwrap();
                                    *guard = Some(item.clone());
                                }

                                // Reset timer to 50 ticks = 5.0 seconds
                                shelf_timer_counter.store(50, Ordering::Relaxed);

                                // Dispatch to Slint UI
                                let thumb_raw = item.thumbnail_rgba;
                                let tw = item.thumb_width;
                                let th = item.thumb_height;
                                let filename = item.filename;
                                let res_text = item.resolution_text;
                                let path_str = item.path.to_string_lossy().to_string();

                                let _ = weak_app.upgrade_in_event_loop(move |app| {
                                    let pixel_buffer = slint::SharedPixelBuffer::clone_from_slice(&thumb_raw, tw, th);
                                    let slint_img = slint::Image::from_rgba8(pixel_buffer);

                                    app.set_shelf_thumb(slint_img);
                                    app.set_shelf_filename(filename.into());
                                    app.set_shelf_res_text(res_text.into());
                                    app.set_shelf_path(path_str.into());
                                    app.set_has_shelf_item(true);
                                    app.set_is_shelf_active(true);
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

fn process_new_screenshot(
    shelf_dir: &Path,
    w: u32,
    h: u32,
    raw_rgba: Vec<u8>,
) -> Option<ShelfItem> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let filename = format!("Kripton_Shot_{}_{}x{}.png", now, w, h);
    let file_path = shelf_dir.join(&filename);

    if let Some(rgba_img) = image::RgbaImage::from_raw(w, h, raw_rgba) {
        // Save full image asynchronously in background so we don't stall
        let save_path = file_path.clone();
        let img_clone = rgba_img.clone();
        std::thread::spawn(move || {
            let _ = img_clone.save(&save_path);
        });

        // Generate thumbnail (max 52x52)
        let thumb = image::imageops::thumbnail(&rgba_img, 52, 52);
        let (tw, th) = thumb.dimensions();
        let thumb_rgba = thumb.into_raw();

        Some(ShelfItem {
            path: file_path,
            filename,
            width: w,
            height: h,
            resolution_text: format!("{} × {}", w, h),
            thumbnail_rgba: thumb_rgba,
            thumb_width: tw,
            thumb_height: th,
        })
    } else {
        None
    }
}

fn get_or_create_shelf_dir() -> PathBuf {
    let user_profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Public".into());
    let pictures_dir = PathBuf::from(user_profile).join("Pictures").join("Screenshots").join("Kripton");

    if std::fs::create_dir_all(&pictures_dir).is_ok() {
        pictures_dir
    } else {
        let temp_dir = std::env::temp_dir().join("Kripton_Shelf");
        let _ = std::fs::create_dir_all(&temp_dir);
        temp_dir
    }
}

fn compute_quick_hash(w: u32, h: u32, bytes: &[u8]) -> u64 {
    let mut hash = (w as u64) ^ ((h as u64) << 16) ^ ((bytes.len() as u64) << 32);
    let step = (bytes.len() / 32).max(1);
    for i in (0..bytes.len()).step_by(step) {
        hash = hash.wrapping_mul(31).wrapping_add(bytes[i] as u64);
    }
    hash
}

pub fn open_file(path: &Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

pub fn copy_to_clipboard(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text);
    }
}
