//! LRCLIB integration, LRC timestamp parser, local disk/memory cache,
//! and live lyrics synchronization.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use serde::{Deserialize, Serialize};
use windows::{core::HSTRING, Foundation::Uri, Web::Http::HttpClient};

use crate::state::APP_DIR_NAME;

const LRCLIB_API_GET: &str = "https://lrclib.net/api/get";
const LRCLIB_API_SEARCH: &str = "https://lrclib.net/api/search";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    pub timestamp_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncedLyrics {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub lines: Vec<LyricLine>,
    pub is_synced: bool,
    pub plain_lyrics: Option<String>,
}

impl SyncedLyrics {
    /// Given a playback position in milliseconds, find the active line index
    /// and surrounding lines (prev, active, next).
    pub fn current_window(&self, position_ms: u64) -> (Option<&str>, Option<&str>, Option<&str>) {
        if self.lines.is_empty() {
            return (None, None, None);
        }

        // Binary search for the last line with timestamp_ms <= position_ms.
        let mut low = 0;
        let mut high = self.lines.len();
        let mut match_idx: Option<usize> = None;

        while low < high {
            let mid = low + (high - low) / 2;
            if self.lines[mid].timestamp_ms <= position_ms {
                match_idx = Some(mid);
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        match match_idx {
            Some(idx) => {
                let prev = idx.checked_sub(1).map(|i| self.lines[i].text.as_str());
                let curr = Some(self.lines[idx].text.as_str());
                let next = self.lines.get(idx + 1).map(|l| l.text.as_str());
                (prev, curr, next)
            }
            None => {
                // Before the first lyric line starts.
                (None, None, self.lines.first().map(|l| l.text.as_str()))
            }
        }
    }
}

/// Parse raw LRC text into a sorted list of `LyricLine`s.
pub fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();

    for raw_line in lrc.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // An LRC line can contain multiple timestamps: [00:12.00][00:24.00]Lyrics
        let mut rest = trimmed;
        let mut timestamps = Vec::new();

        while rest.starts_with('[') {
            if let Some(close_bracket) = rest.find(']') {
                let tag = &rest[1..close_bracket];
                if let Some(ms) = parse_lrc_timestamp(tag) {
                    timestamps.push(ms);
                }
                rest = rest[close_bracket + 1..].trim_start();
            } else {
                break;
            }
        }

        let lyric_text = rest.trim();
        for ts in timestamps {
            lines.push(LyricLine {
                timestamp_ms: ts,
                text: lyric_text.to_string(),
            });
        }
    }

    lines.sort_by_key(|l| l.timestamp_ms);
    lines
}

/// Parse `mm:ss.xx` or `mm:ss.xxx` or `mm:ss` into milliseconds.
fn parse_lrc_timestamp(tag: &str) -> Option<u64> {
    let parts: Vec<&str> = tag.split(':').collect();
    if parts.len() != 2 {
        return None;
    }

    let minutes: u64 = parts[0].parse().ok()?;
    let sec_parts: Vec<&str> = parts[1].split('.').collect();

    let (seconds, ms) = if sec_parts.len() == 2 {
        let sec: u64 = sec_parts[0].parse().ok()?;
        let frac_str = sec_parts[1];
        let frac_val: u64 = frac_str.parse().ok()?;
        let ms = match frac_str.len() {
            1 => frac_val * 100,
            2 => frac_val * 10,
            3 => frac_val,
            _ => frac_str[..3].parse().ok()?,
        };
        (sec, ms)
    } else if sec_parts.len() == 1 {
        let sec: u64 = sec_parts[0].parse().ok()?;
        (sec, 0)
    } else {
        return None;
    };

    Some(minutes * 60_000 + seconds * 1_000 + ms)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibSearchResult {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// Disk and in-memory cache manager for lyrics.
#[derive(Clone)]
pub struct LyricsManager {
    inner: Arc<LyricsManagerInner>,
}

struct LyricsManagerInner {
    memory_cache: Mutex<HashMap<String, Option<SyncedLyrics>>>,
    cache_dir: PathBuf,
    client: HttpClient,
    in_flight: Mutex<HashMap<String, bool>>,
}

impl Default for LyricsManager {
    fn default() -> Self {
        Self::new().expect("failed to initialize LyricsManager")
    }
}

impl LyricsManager {
    pub fn new() -> Option<Self> {
        let client = HttpClient::new().ok()?;
        let cache_dir = directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().join(APP_DIR_NAME).join("lyrics_cache"))
            .unwrap_or_else(|| PathBuf::from("lyrics_cache"));

        let _ = fs::create_dir_all(&cache_dir);

        Some(Self {
            inner: Arc::new(LyricsManagerInner {
                memory_cache: Mutex::new(HashMap::new()),
                cache_dir,
                client,
                in_flight: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn track_key(artist: &str, title: &str) -> String {
        let a = artist.trim().to_lowercase();
        let t = title.trim().to_lowercase();
        format!("{a} - {t}")
    }

    /// Retrieve lyrics for a track. If cached, returns immediately.
    /// If not cached, triggers an asynchronous background query.
    pub fn get_or_fetch(
        &self,
        artist: &str,
        title: &str,
        duration_ms: u64,
    ) -> Option<SyncedLyrics> {
        if title.trim().is_empty() {
            return None;
        }

        let key = Self::track_key(artist, title);

        // 1. Check in-memory cache.
        if let Ok(cache) = self.inner.memory_cache.lock() {
            if let Some(entry) = cache.get(&key) {
                return entry.clone();
            }
        }

        // 2. Check disk cache.
        let file_path = self.disk_path(&key);
        if file_path.exists() {
            if let Ok(file) = File::open(&file_path) {
                let reader = BufReader::new(file);
                if let Ok(lyrics) = serde_json::from_reader::<_, SyncedLyrics>(reader) {
                    if let Ok(mut cache) = self.inner.memory_cache.lock() {
                        cache.insert(key.clone(), Some(lyrics.clone()));
                    }
                    return Some(lyrics);
                }
            }
        }

        // 3. Mark in-flight and fetch in background thread.
        let should_fetch = {
            let mut in_flight = self.inner.in_flight.lock().unwrap();
            if in_flight.contains_key(&key) {
                false
            } else {
                in_flight.insert(key.clone(), true);
                true
            }
        };

        if should_fetch {
            let artist_owned = artist.to_string();
            let title_owned = title.to_string();
            let key_clone = key.clone();
            let inner_clone = self.inner.clone();

            thread::Builder::new()
                .name("kripton-lyrics-fetcher".into())
                .spawn(move || {
                    let result = fetch_lrclib(
                        &inner_clone.client,
                        &artist_owned,
                        &title_owned,
                        duration_ms / 1000,
                    );

                    let lyrics = result.and_then(|res| {
                        if let Some(synced) = res.synced_lyrics {
                            let lines = parse_lrc(&synced);
                            if !lines.is_empty() {
                                return Some(SyncedLyrics {
                                    track_id: key_clone.clone(),
                                    title: title_owned.clone(),
                                    artist: artist_owned.clone(),
                                    lines,
                                    is_synced: true,
                                    plain_lyrics: res.plain_lyrics,
                                });
                            }
                        }

                        if let Some(plain) = res.plain_lyrics {
                            let dummy_lines = plain
                                .lines()
                                .map(|l| l.trim())
                                .filter(|l| !l.is_empty())
                                .enumerate()
                                .map(|(idx, line)| LyricLine {
                                    timestamp_ms: (idx as u64) * 3000,
                                    text: line.to_string(),
                                })
                                .collect();

                            return Some(SyncedLyrics {
                                track_id: key_clone.clone(),
                                title: title_owned.clone(),
                                artist: artist_owned.clone(),
                                lines: dummy_lines,
                                is_synced: false,
                                plain_lyrics: Some(plain),
                            });
                        }

                        None
                    });

                    // Save to disk cache if found.
                    if let Some(ref lyrics_data) = lyrics {
                        let path = inner_clone
                            .cache_dir
                            .join(format!("{}.json", sanitize_filename(&key_clone)));
                        if let Ok(mut file) = File::create(path) {
                            if let Ok(json) = serde_json::to_string(lyrics_data) {
                                let _ = file.write_all(json.as_bytes());
                            }
                        }
                    }

                    // Update memory cache
                    if let Ok(mut guard) = inner_clone.memory_cache.lock() {
                        guard.insert(key_clone.clone(), lyrics);
                    }
                    if let Ok(mut in_flight) = inner_clone.in_flight.lock() {
                        in_flight.remove(&key_clone);
                    }
                })
                .expect("failed to spawn lyrics fetcher thread");
        }

        None
    }

    fn disk_path(&self, key: &str) -> PathBuf {
        self.inner
            .cache_dir
            .join(format!("{}.json", sanitize_filename(key)))
    }
}

fn fetch_lrclib(
    client: &HttpClient,
    artist: &str,
    title: &str,
    duration_secs: u64,
) -> Option<LrclibResponse> {
    // 1. Try /api/get with exact parameters
    let get_url = format!(
        "{}?track_name={}&artist_name={}&duration={}",
        LRCLIB_API_GET,
        url_encode(title),
        url_encode(artist),
        duration_secs
    );

    if let Ok(body) = http_get(client, &get_url) {
        if let Ok(resp) = serde_json::from_str::<LrclibResponse>(&body) {
            if resp.synced_lyrics.is_some() || resp.plain_lyrics.is_some() {
                return Some(resp);
            }
        }
    }

    // 2. Fallback to /api/search?q=artist+title
    let query = format!("{} {}", artist, title);
    let search_url = format!("{}?q={}", LRCLIB_API_SEARCH, url_encode(&query));

    if let Ok(body) = http_get(client, &search_url) {
        if let Ok(results) = serde_json::from_str::<Vec<LrclibSearchResult>>(&body) {
            for item in results {
                if item.synced_lyrics.is_some() || item.plain_lyrics.is_some() {
                    return Some(LrclibResponse {
                        synced_lyrics: item.synced_lyrics,
                        plain_lyrics: item.plain_lyrics,
                    });
                }
            }
        }
    }

    None
}

fn http_get(client: &HttpClient, url: &str) -> Result<String, windows::core::Error> {
    use std::future::IntoFuture;
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    let async_op = client.GetStringAsync(&uri)?;
    let hstr = crate::media::block_on(async_op.into_future())?;
    Ok(hstr.to_string())
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lrc_standard() {
        let lrc = r#"
[ti:Yellow]
[ar:Coldplay]
[00:15.30]Look at the stars
[00:18.50]Look how they shine for you
[00:23.00]And everything you do
"#;

        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].timestamp_ms, 15300);
        assert_eq!(lines[0].text, "Look at the stars");
        assert_eq!(lines[1].timestamp_ms, 18500);
        assert_eq!(lines[1].text, "Look how they shine for you");
        assert_eq!(lines[2].timestamp_ms, 23000);
        assert_eq!(lines[2].text, "And everything you do");
    }

    #[test]
    fn test_parse_lrc_multi_timestamp() {
        let lrc = "[00:10.00][00:20.00]Chorus line";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].timestamp_ms, 10000);
        assert_eq!(lines[0].text, "Chorus line");
        assert_eq!(lines[1].timestamp_ms, 20000);
        assert_eq!(lines[1].text, "Chorus line");
    }

    #[test]
    fn test_current_window() {
        let synced = SyncedLyrics {
            track_id: "test".into(),
            title: "Test".into(),
            artist: "Artist".into(),
            lines: vec![
                LyricLine {
                    timestamp_ms: 5000,
                    text: "Line 1".into(),
                },
                LyricLine {
                    timestamp_ms: 10000,
                    text: "Line 2".into(),
                },
                LyricLine {
                    timestamp_ms: 15000,
                    text: "Line 3".into(),
                },
            ],
            is_synced: true,
            plain_lyrics: None,
        };

        // Before first line
        let (prev, curr, next) = synced.current_window(2000);
        assert_eq!(prev, None);
        assert_eq!(curr, None);
        assert_eq!(next, Some("Line 1"));

        // Exactly at Line 1
        let (prev, curr, next) = synced.current_window(5000);
        assert_eq!(prev, None);
        assert_eq!(curr, Some("Line 1"));
        assert_eq!(next, Some("Line 2"));

        // During Line 2
        let (prev, curr, next) = synced.current_window(12000);
        assert_eq!(prev, Some("Line 1"));
        assert_eq!(curr, Some("Line 2"));
        assert_eq!(next, Some("Line 3"));

        // During Line 3 (last line)
        let (prev, curr, next) = synced.current_window(18000);
        assert_eq!(prev, Some("Line 2"));
        assert_eq!(curr, Some("Line 3"));
        assert_eq!(next, None);
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("Hello World"), "Hello+World");
        assert_eq!(url_encode("Coldplay & BTS"), "Coldplay+%26+BTS");
    }
}
