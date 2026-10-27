//! Sprint 5: monotonic Pomodoro timer and the completion chime.
//!
//! The countdown is anchored to `GetTickCount64`, which keeps counting while
//! the machine is suspended. A plain `Instant` can stall across sleep on some
//! Windows configurations, so the deadline is stored in tick milliseconds.

use windows::{
    core::PCWSTR,
    Win32::{
        Media::Audio::{PlaySoundW, SND_ASYNC, SND_FLAGS, SND_MEMORY, SND_NODEFAULT},
        System::SystemInformation::GetTickCount64,
    },
};

pub const BREAK_MINUTES: u32 = 5;
pub const MIN_FOCUS_MINUTES: u32 = 5;
pub const MAX_FOCUS_MINUTES: u32 = 120;

#[repr(align(4))]
struct ChimeWave([u8; include_bytes!("../assets/chime.wav").len()]);

static CHIME: ChimeWave = ChimeWave(*include_bytes!("../assets/chime.wav"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPhase {
    Focus,
    Break,
}

impl FocusPhase {
    fn duration_ms(self, focus_minutes: u32) -> u64 {
        let minutes = match self {
            FocusPhase::Focus => u64::from(focus_minutes),
            FocusPhase::Break => u64::from(BREAK_MINUTES),
        };
        minutes * 60_000
    }
}

#[derive(Debug, Clone)]
pub struct FocusTimer {
    phase: FocusPhase,
    focus_minutes: u32,
    total_ms: u64,
    remaining_ms: u64,
    running: bool,
    deadline_tick: u64,
}

impl FocusTimer {
    pub fn new(focus_minutes: u32) -> Self {
        let focus_minutes = focus_minutes.clamp(MIN_FOCUS_MINUTES, MAX_FOCUS_MINUTES);
        let total_ms = FocusPhase::Focus.duration_ms(focus_minutes);
        Self {
            phase: FocusPhase::Focus,
            focus_minutes,
            total_ms,
            remaining_ms: total_ms,
            running: false,
            deadline_tick: 0,
        }
    }

    pub fn phase(&self) -> FocusPhase {
        self.phase
    }

    pub fn focus_minutes(&self) -> u32 {
        self.focus_minutes
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn progress(&self) -> f32 {
        if self.total_ms == 0 {
            return 0.0;
        }
        (1.0 - self.remaining_ms as f32 / self.total_ms as f32).clamp(0.0, 1.0)
    }

    pub fn remaining_text(&self) -> String {
        format_remaining(self.remaining_ms)
    }

    /// A timer is "active" while it runs or while a phase has already been
    /// partially consumed, so the compact pill can show the leftover time.
    pub fn active(&self) -> bool {
        self.running || self.remaining_ms < self.total_ms
    }

    pub fn start(&mut self) {
        if self.remaining_ms == 0 {
            self.remaining_ms = self.total_ms;
        }
        self.deadline_tick = tick_now().saturating_add(self.remaining_ms);
        self.running = true;
    }

    pub fn pause(&mut self) {
        self.sync();
        self.running = false;
    }

    pub fn toggle(&mut self) {
        if self.running {
            self.pause();
        } else {
            self.start();
        }
    }

    pub fn reset(&mut self) {
        self.running = false;
        self.remaining_ms = self.total_ms;
    }

    /// Changing the focus duration resets the current focus countdown. It is
    /// ignored during a break so the break keeps its five minutes.
    pub fn set_focus_minutes(&mut self, minutes: u32) {
        self.focus_minutes = minutes.clamp(MIN_FOCUS_MINUTES, MAX_FOCUS_MINUTES);
        if self.phase == FocusPhase::Focus {
            self.total_ms = FocusPhase::Focus.duration_ms(self.focus_minutes);
            self.reset();
        }
    }

    pub fn adjust_focus_minutes(&mut self, delta: i32) {
        let next = i64::from(self.focus_minutes) + i64::from(delta);
        self.set_focus_minutes(next.clamp(0, i64::from(MAX_FOCUS_MINUTES)) as u32);
    }

    /// Advances the countdown and reports the phase that just finished. The
    /// caller is responsible for the chime and for calling [`Self::complete`].
    pub fn tick(&mut self) -> Option<FocusPhase> {
        if !self.running {
            return None;
        }
        let now = tick_now();
        if now >= self.deadline_tick {
            self.remaining_ms = 0;
            self.running = false;
            return Some(self.phase);
        }
        self.remaining_ms = self.deadline_tick - now;
        None
    }

    /// Switches to the next phase, paused at its full duration, and returns the
    /// phase that was completed.
    pub fn complete(&mut self) -> FocusPhase {
        let completed = self.phase;
        self.phase = match completed {
            FocusPhase::Focus => FocusPhase::Break,
            FocusPhase::Break => FocusPhase::Focus,
        };
        self.total_ms = self.phase.duration_ms(self.focus_minutes);
        self.remaining_ms = self.total_ms;
        self.running = false;
        completed
    }

    fn sync(&mut self) {
        if !self.running {
            return;
        }
        let now = tick_now();
        self.remaining_ms = self.deadline_tick.saturating_sub(now);
        if self.remaining_ms == 0 {
            self.running = false;
        }
    }
}

pub fn play_chime() {
    let flags = SND_FLAGS(SND_ASYNC.0 | SND_MEMORY.0 | SND_NODEFAULT.0);
    unsafe {
        let _ = PlaySoundW(PCWSTR(CHIME.0.as_ptr() as *const u16), None, flags);
    }
}

fn tick_now() -> u64 {
    unsafe { GetTickCount64() }
}

fn format_remaining(remaining_ms: u64) -> String {
    // Round up so the display starts at 25:00 and only shows 00:00 when the
    // phase is actually over.
    let seconds = remaining_ms.div_ceil(1000);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_text_rounds_up() {
        assert_eq!(format_remaining(25 * 60_000), "25:00");
        assert_eq!(format_remaining(24 * 60_000 + 59_000), "24:59");
        assert_eq!(format_remaining(999), "00:01");
        assert_eq!(format_remaining(0), "00:00");
    }

    #[test]
    fn focus_duration_is_clamped() {
        let mut timer = FocusTimer::new(1);
        assert_eq!(timer.focus_minutes(), MIN_FOCUS_MINUTES);
        timer.set_focus_minutes(500);
        assert_eq!(timer.focus_minutes(), MAX_FOCUS_MINUTES);
        assert_eq!(timer.remaining_text(), "120:00");

        timer.adjust_focus_minutes(-1);
        assert_eq!(timer.focus_minutes(), 119);
        timer.adjust_focus_minutes(-500);
        assert_eq!(timer.focus_minutes(), MIN_FOCUS_MINUTES);
    }

    #[test]
    fn pause_preserves_the_remaining_time() {
        let mut timer = FocusTimer::new(25);
        assert!(!timer.active());
        timer.start();
        assert!(timer.running());
        assert!(timer.active());
        timer.pause();
        assert!(!timer.running());
        assert_eq!(timer.remaining_text(), "25:00");
        assert_eq!(timer.progress(), 0.0);
    }

    #[test]
    fn completing_a_focus_session_switches_to_a_short_break() {
        let mut timer = FocusTimer::new(25);
        timer.start();
        let completed = timer.complete();
        assert_eq!(completed, FocusPhase::Focus);
        assert_eq!(timer.phase(), FocusPhase::Break);
        assert_eq!(timer.remaining_text(), "05:00");
        assert!(!timer.running());

        let completed = timer.complete();
        assert_eq!(completed, FocusPhase::Break);
        assert_eq!(timer.phase(), FocusPhase::Focus);
        assert_eq!(timer.remaining_text(), "25:00");
    }
}
