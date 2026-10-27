//! Sprint 4: native OLE drop target and the file/folder shelf.
//!
//! Explorer hands us a `CF_HDROP` payload through `IDropTarget`. The drop
//! callback runs on the UI (STA) thread, so it only records the paths and
//! flips a couple of flags; the 16 ms runtime tick drains the queue, updates
//! the Slint model and persists `state.json`. Thumbnails are decoded on a
//! small worker thread so a large screenshot never stalls the notch.

use std::{
    collections::{HashMap, HashSet},
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
};

use windows::{
    core::{implement, w, Ref, Result as WinResult, PCWSTR},
    Win32::{
        Foundation::{HWND, POINTL},
        System::{
            Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
            Ole::{
                IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
                CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
            },
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::{
            Shell::{DragQueryFileW, ShellExecuteW, HDROP},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

use crate::state::{ShelfItem as PersistedShelfItem, StateStore};

pub const MAX_ITEMS: usize = 20;
pub const VISIBLE_ITEMS: usize = 5;
const THUMB_SIZE: u32 = 108;
const MAX_THUMB_SOURCE_BYTES: u64 = 40 * 1024 * 1024;
const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Clone)]
pub struct ShelfEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub size_text: String,
    pub missing: bool,
}

#[derive(Debug, Clone)]
pub struct ShelfThumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct ShelfSnapshot {
    pub items: Vec<ShelfEntry>,
    pub thumbs: HashMap<PathBuf, ShelfThumb>,
    pub revision: u64,
}

#[derive(Default)]
struct ShelfShared {
    items: Vec<ShelfEntry>,
    thumbs: HashMap<PathBuf, ShelfThumb>,
    requested: HashSet<PathBuf>,
    pending: Vec<PathBuf>,
    revision: u64,
    drag_active: bool,
    open_requested: bool,
}

/// UI-side handle. Cloning shares the same shelf and persistence store.
#[derive(Clone)]
pub struct ShelfManager {
    shared: Arc<Mutex<ShelfShared>>,
    store: StateStore,
    jobs: Sender<PathBuf>,
}

impl ShelfManager {
    pub fn start(store: StateStore) -> Self {
        let shared = Arc::new(Mutex::new(ShelfShared::default()));
        let (jobs, receiver) = mpsc::channel();

        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut cleaned = false;

        for saved in store.with(|state| state.shelf_items.clone()) {
            let path = PathBuf::from(&saved.path);
            let key = path_key(&path);
            if !seen.insert(key) {
                cleaned = true;
                continue;
            }
            match fs::metadata(&path) {
                Ok(metadata) => items.push(entry_from_metadata(path, &metadata)),
                Err(_) => cleaned = true,
            }
        }

        if items.len() > MAX_ITEMS {
            items.drain(0..items.len() - MAX_ITEMS);
            cleaned = true;
        }

        {
            let mut shared = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            shared.items = items;
            shared.revision = 1;
        }

        let worker_shared = shared.clone();
        thread::Builder::new()
            .name("kripton-shelf-thumbs".into())
            .spawn(move || thumb_worker(receiver, worker_shared))
            .expect("failed to spawn shelf thumbnail worker");

        let manager = Self {
            shared,
            store,
            jobs,
        };

        if cleaned {
            manager.persist();
        }
        manager.queue_missing_thumbnails();
        manager
    }

    pub fn revision(&self) -> u64 {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .revision
    }

    pub fn len(&self) -> usize {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .items
            .len()
    }

    #[allow(dead_code)] // Kept alongside `len` for the usual container API.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> ShelfSnapshot {
        let (mut items, thumbs, revision) = {
            let shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (shared.items.clone(), shared.thumbs.clone(), shared.revision)
        };
        for item in &mut items {
            item.missing = !item.path.exists();
        }
        ShelfSnapshot {
            items,
            thumbs,
            revision,
        }
    }

    pub fn item_path(&self, index: usize) -> Option<PathBuf> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .items
            .get(index)
            .map(|item| item.path.clone())
    }

    pub fn take_pending(&self) -> Vec<PathBuf> {
        std::mem::take(
            &mut self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pending,
        )
    }

    pub fn add_paths(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }

        let mut changed = false;
        let mut new_images = Vec::new();
        {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            for path in paths {
                let Ok(metadata) = fs::metadata(&path) else {
                    continue;
                };
                if shared.items.iter().any(|item| same_path(&item.path, &path)) {
                    continue;
                }

                shared
                    .items
                    .push(entry_from_metadata(path.clone(), &metadata));
                changed = true;

                if is_image_path(&path)
                    && !shared.thumbs.contains_key(&path)
                    && shared.requested.insert(path.clone())
                {
                    new_images.push(path);
                }
            }

            if shared.items.len() > MAX_ITEMS {
                let excess = shared.items.len() - MAX_ITEMS;
                shared.items.drain(0..excess);
            }

            if changed {
                // Drop cached thumbnails for entries that fell off the shelf.
                let keep: HashSet<String> = shared
                    .items
                    .iter()
                    .map(|item| path_key(&item.path))
                    .collect();
                shared
                    .thumbs
                    .retain(|path, _| keep.contains(&path_key(path)));
                shared.revision = shared.revision.wrapping_add(1);
            }
        }

        for path in new_images {
            let _ = self.jobs.send(path);
        }
        if changed {
            self.persist();
        }
    }

    pub fn remove(&self, index: usize) {
        let mut changed = false;
        {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if index < shared.items.len() {
                shared.items.remove(index);
                shared.revision = shared.revision.wrapping_add(1);
                changed = true;
            }
        }
        if changed {
            self.persist();
        }
    }

    pub fn clear(&self) {
        let mut changed = false;
        {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !shared.items.is_empty() {
                shared.items.clear();
                shared.thumbs.clear();
                shared.requested.clear();
                shared.revision = shared.revision.wrapping_add(1);
                changed = true;
            }
        }
        if changed {
            self.persist();
        }
    }

    pub fn drag_active(&self) -> bool {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drag_active
    }

    pub fn take_open_request(&self) -> bool {
        std::mem::take(
            &mut self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .open_requested,
        )
    }

    fn on_drag_enter(&self) {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.drag_active = true;
        shared.open_requested = true;
    }

    fn on_drag_leave(&self) {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drag_active = false;
    }

    fn on_drop(&self, paths: Vec<PathBuf>) {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.drag_active = false;
        shared.open_requested = true;
        shared.pending.extend(paths);
    }

    fn queue_missing_thumbnails(&self) {
        let paths = {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let candidates: Vec<PathBuf> = shared
                .items
                .iter()
                .filter(|item| is_image_path(&item.path))
                .map(|item| item.path.clone())
                .collect();
            let mut missing = Vec::new();
            for path in candidates {
                if shared.thumbs.contains_key(&path) {
                    continue;
                }
                if shared.requested.insert(path.clone()) {
                    missing.push(path);
                }
            }
            missing
        };
        for path in paths {
            let _ = self.jobs.send(path);
        }
    }

    fn persist(&self) {
        let items = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .items
            .clone();
        self.store.update(|state| {
            state.shelf_items = items
                .iter()
                .map(|item| PersistedShelfItem {
                    name: item.name.clone(),
                    path: item.path.to_string_lossy().into_owned(),
                    is_directory: item.is_directory,
                    size_bytes: item.size_bytes,
                    size_formatted: item.size_text.clone(),
                })
                .collect();
        });
    }
}

/// Register the drop target on the notch HWND. Must be called on the OLE STA
/// (the Slint/UI thread). `RegisterDragDrop` takes its own reference, so the
/// local interface can be dropped immediately.
pub fn register_drop_target(hwnd: HWND, shelf: ShelfManager) -> WinResult<()> {
    let target: IDropTarget = ShelfDropTarget { shelf }.into();
    unsafe {
        // winit registers its own IDropTarget on every window for
        // `WindowEvent::DroppedFile`. We do not consume those events, so take
        // the slot over; otherwise RegisterDragDrop fails with
        // DRAGDROP_E_ALREADYREGISTERED (0x80040101).
        let _ = RevokeDragDrop(hwnd);
        RegisterDragDrop(hwnd, &target)
    }
}

pub fn open_path(path: &Path) {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

pub fn clamp_start(start: usize, len: usize) -> usize {
    start.min(len.saturating_sub(VISIBLE_ITEMS))
}

pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f < KB {
        format!("{bytes} B")
    } else if bytes_f < MB {
        format!("{:.1} KB", bytes_f / KB)
    } else if bytes_f < GB {
        format!("{:.1} MB", bytes_f / MB)
    } else {
        format!("{:.2} GB", bytes_f / GB)
    }
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(false)
}

fn entry_from_metadata(path: PathBuf, metadata: &fs::Metadata) -> ShelfEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let is_directory = metadata.is_dir();
    let size_bytes = if is_directory { 0 } else { metadata.len() };
    let size_text = if is_directory {
        "Folder".to_string()
    } else {
        format_size(size_bytes)
    };

    ShelfEntry {
        path,
        name,
        is_directory,
        size_bytes,
        size_text,
        missing: false,
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

fn thumb_worker(receiver: Receiver<PathBuf>, shared: Arc<Mutex<ShelfShared>>) {
    while let Ok(path) = receiver.recv() {
        let Some(thumb) = load_thumb(&path) else {
            continue;
        };
        let mut shared = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.thumbs.insert(path, thumb);
        shared.revision = shared.revision.wrapping_add(1);
    }
}

fn load_thumb(path: &Path) -> Option<ShelfThumb> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_THUMB_SOURCE_BYTES {
        return None;
    }

    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let decoded = reader.decode().ok()?;
    let thumb = image::imageops::thumbnail(&decoded, THUMB_SIZE, THUMB_SIZE);
    let width = thumb.width();
    let height = thumb.height();
    Some(ShelfThumb {
        width,
        height,
        rgba: thumb.into_raw(),
    })
}

#[implement(IDropTarget)]
struct ShelfDropTarget {
    shelf: ShelfManager,
}

impl IDropTarget_Impl for ShelfDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _key_state: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let has_files = data.as_ref().map(data_has_files).unwrap_or(false);
        unsafe {
            if !effect.is_null() {
                *effect = if has_files {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
        }
        if has_files {
            self.shelf.on_drag_enter();
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _key_state: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        unsafe {
            if !effect.is_null() {
                *effect = DROPEFFECT_COPY;
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        self.shelf.on_drag_leave();
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _key_state: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let paths = data.as_ref().map(paths_from_data).unwrap_or_default();
        crate::log_line(&format!("shelf drop received: {} path(s)", paths.len()));
        unsafe {
            if !effect.is_null() {
                *effect = DROPEFFECT_COPY;
            }
        }
        self.shelf.on_drop(paths);
        Ok(())
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn data_has_files(data: &IDataObject) -> bool {
    unsafe { data.QueryGetData(&hdrop_format()).is_ok() }
}

fn paths_from_data(data: &IDataObject) -> Vec<PathBuf> {
    let mut medium = match unsafe { data.GetData(&hdrop_format()) } {
        Ok(medium) => medium,
        Err(_) => return Vec::new(),
    };

    let mut paths = Vec::new();
    unsafe {
        let hdrop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for index in 0..count {
            let length = DragQueryFileW(hdrop, index, None);
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = DragQueryFileW(hdrop, index, Some(&mut buffer));
            if written == 0 {
                continue;
            }
            buffer.truncate(written as usize);
            paths.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
        }
        ReleaseStgMedium(&mut medium);
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formatting_matches_examples() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2_516_582), "2.4 MB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.00 GB");
    }

    #[test]
    fn carousel_start_is_clamped() {
        assert_eq!(clamp_start(0, 0), 0);
        assert_eq!(clamp_start(3, 2), 0);
        assert_eq!(clamp_start(0, 5), 0);
        assert_eq!(clamp_start(1, 7), 1);
        assert_eq!(clamp_start(9, 7), 2);
        assert_eq!(clamp_start(9, 20), 9);
        assert_eq!(clamp_start(99, 20), 15);
    }

    #[test]
    fn image_extensions_are_detected_case_insensitively() {
        assert!(is_image_path(Path::new("C:\\shots\\beach.PNG")));
        assert!(is_image_path(Path::new("photo.jpeg")));
        assert!(is_image_path(Path::new("art.webp")));
        assert!(!is_image_path(Path::new("notes.txt")));
        assert!(!is_image_path(Path::new("folder")));
    }

    #[test]
    fn path_comparison_ignores_case() {
        assert!(same_path(
            Path::new("C:\\Users\\Dell\\Beach.JPG"),
            Path::new("c:\\users\\dell\\beach.jpg")
        ));
    }
}
