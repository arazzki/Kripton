use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Local;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

pub const APP_DIR_NAME: &str = "Kripton";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Music,
    Focus,
    Water,
    Shelf,
    Todo,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Music,
        Page::Focus,
        Page::Water,
        Page::Shelf,
        Page::Todo,
    ];

    pub fn from_index(index: u8) -> Self {
        Self::ALL
            .get(index as usize)
            .copied()
            .unwrap_or(Page::Music)
    }

    pub fn index(self) -> u8 {
        match self {
            Page::Music => 0,
            Page::Focus => 1,
            Page::Water => 2,
            Page::Shelf => 3,
            Page::Todo => 4,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Music => "Music",
            Page::Focus => "Focus",
            Page::Water => "Water",
            Page::Shelf => "Shelf",
            Page::Todo => "To-do",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Page::Music => "SMTC media · live spectrum",
            Page::Focus => "Pomodoro focus timer",
            Page::Water => "Daily hydration tracker",
            Page::Shelf => "Drag & drop file shelf",
            Page::Todo => "Quick task checklist",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Closed/Lyrics/Hud are wired in later sprints.
pub enum NotchMode {
    Idle,
    Closed,
    Expanded,
    Lyrics,
    Hud,
}

impl NotchMode {
    pub fn size(self) -> (f32, f32) {
        match self {
            NotchMode::Idle => (190.0, 38.0),
            NotchMode::Closed => (210.0, 38.0),
            NotchMode::Expanded => (460.0, 175.0),
            NotchMode::Lyrics => (560.0, 165.0),
            NotchMode::Hud => (320.0, 64.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FocusTimerState {
    pub default_minutes: u32,
    pub completed_sessions_today: u32,
    pub last_session_date: String,
}

impl Default for FocusTimerState {
    fn default() -> Self {
        Self {
            default_minutes: 25,
            completed_sessions_today: 0,
            last_session_date: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterTrackerState {
    pub date: String,
    pub current_ml: u32,
    pub daily_goal_ml: u32,
    pub last_drink_timestamp: i64,
}

impl Default for WaterTrackerState {
    fn default() -> Self {
        Self {
            date: String::new(),
            current_ml: 0,
            daily_goal_ml: 2000,
            last_drink_timestamp: 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShelfItem {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub size_formatted: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoItem {
    pub id: u64,
    pub text: String,
    pub completed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    pub active_page: u8,
    pub lyrics_enabled: bool,
    pub focus_timer: FocusTimerState,
    pub water_tracker: WaterTrackerState,
    pub shelf_items: Vec<ShelfItem>,
    pub todos: Vec<TodoItem>,
}

impl PersistedState {
    pub fn page(&self) -> Page {
        Page::from_index(self.active_page)
    }

    #[allow(dead_code)] // Consumed by the to-do page in Sprint 5.
    pub fn next_todo_id(&self) -> u64 {
        self.todos.iter().map(|todo| todo.id).max().unwrap_or(0) + 1
    }

    /// Reset per-day counters when the local date changes. Returns true when the
    /// state was modified and should be written back to disk.
    fn normalize_for_today(&mut self) -> bool {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let mut changed = false;

        if self.focus_timer.last_session_date != today {
            self.focus_timer.last_session_date = today.clone();
            self.focus_timer.completed_sessions_today = 0;
            changed = true;
        }

        if self.water_tracker.date != today {
            self.water_tracker.date = today;
            self.water_tracker.current_ml = 0;
            self.water_tracker.last_drink_timestamp = 0;
            changed = true;
        }

        changed
    }
}

/// Thread-safe JSON persistence for the whole application.
#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    inner: Arc<Mutex<PersistedState>>,
}

impl StateStore {
    pub fn load() -> Self {
        let path = state_path();
        let mut state = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<PersistedState>(&bytes) {
                Ok(state) => state,
                Err(error) => {
                    eprintln!("[Kripton] state.json is invalid ({error}); starting with defaults");
                    quarantine_corrupt_state(&path);
                    PersistedState::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
            Err(error) => {
                eprintln!("[Kripton] failed to read state.json: {error}");
                PersistedState::default()
            }
        };

        if state.normalize_for_today() {
            // Persist the daily rollover immediately so a crash does not revive
            // yesterday's water/focus counters.
            let _ = write_state(&path, &state);
        }

        Self {
            path,
            inner: Arc::new(Mutex::new(state)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn with<R>(&self, read: impl FnOnce(&PersistedState) -> R) -> R {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&guard)
    }

    pub fn update<R>(&self, update: impl FnOnce(&mut PersistedState) -> R) -> R {
        let result = {
            let mut guard = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            update(&mut guard)
        };
        if let Err(error) = self.save() {
            eprintln!("[Kripton] failed to save state.json: {error}");
        }
        result
    }

    pub fn save(&self) -> std::io::Result<()> {
        let state = {
            let guard = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.clone()
        };
        write_state(&self.path, &state)
    }
}

pub fn state_path() -> PathBuf {
    let base = BaseDirs::new()
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    base.join(APP_DIR_NAME).join("state.json")
}

fn write_state(path: &Path, state: &PersistedState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)?;

    if path.exists() {
        // std::fs::rename does not replace an existing destination on Windows.
        // Removing first keeps the operation effectively atomic for a tiny file.
        let _ = fs::remove_file(path);
    }
    fs::rename(temporary, path)
}

fn quarantine_corrupt_state(path: &Path) {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let backup = path.with_file_name(format!("state.corrupt-{stamp}.json"));
    let _ = fs::rename(path, backup);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_index_round_trip() {
        for page in Page::ALL {
            assert_eq!(Page::from_index(page.index()), page);
        }
    }

    #[test]
    fn notch_sizes_match_spec() {
        assert_eq!(NotchMode::Idle.size(), (190.0, 38.0));
        assert_eq!(NotchMode::Closed.size(), (210.0, 38.0));
        assert_eq!(NotchMode::Expanded.size(), (460.0, 175.0));
        assert_eq!(NotchMode::Lyrics.size(), (560.0, 165.0));
        assert_eq!(NotchMode::Hud.size(), (320.0, 64.0));
    }
}
