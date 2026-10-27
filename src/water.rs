//! Sprint 5: daily hydration tracker.
//!
//! The tracker is intentionally tiny: the persisted counters live in
//! `state.json`, while the runtime keeps the undo slot and the nudge state.

pub const NUDGE_AFTER_SECONDS: i64 = 90 * 60;

#[derive(Debug, Clone)]
pub struct WaterTracker {
    current_ml: u32,
    goal_ml: u32,
    last_drink: i64,
    last_added: u32,
    undo_available: bool,
}

impl WaterTracker {
    pub fn new(current_ml: u32, goal_ml: u32, last_drink: i64) -> Self {
        Self {
            current_ml,
            goal_ml: goal_ml.max(1),
            last_drink,
            last_added: 0,
            undo_available: false,
        }
    }

    pub fn current_ml(&self) -> u32 {
        self.current_ml
    }

    pub fn goal_ml(&self) -> u32 {
        self.goal_ml
    }

    pub fn set_goal_ml(&mut self, goal_ml: u32) {
        self.goal_ml = goal_ml.max(1);
    }

    pub fn last_drink(&self) -> i64 {
        self.last_drink
    }

    pub fn undo_available(&self) -> bool {
        self.undo_available
    }

    pub fn add(&mut self, ml: u32, now: i64) {
        if ml == 0 {
            return;
        }
        self.current_ml = self.current_ml.saturating_add(ml);
        self.last_added = ml;
        self.undo_available = true;
        self.last_drink = now;
    }

    pub fn undo(&mut self) -> bool {
        if !self.undo_available {
            return false;
        }
        self.current_ml = self.current_ml.saturating_sub(self.last_added);
        self.undo_available = false;
        self.last_added = 0;
        true
    }

    pub fn progress(&self) -> f32 {
        (self.current_ml as f32 / self.goal_ml as f32).clamp(0.0, 1.0)
    }

    pub fn percent_text(&self) -> String {
        format!("{}%", (self.progress() * 100.0).round() as u32)
    }

    pub fn summary_text(&self) -> String {
        format!(
            "{} / {} ml",
            format_thousands(self.current_ml),
            format_thousands(self.goal_ml)
        )
    }

    /// True when the user has not logged a drink for the configured interval.
    /// A zero `last_drink` means "no data yet", so a fresh launch never nags.
    pub fn needs_nudge(&self, now: i64, interval_seconds: i64) -> bool {
        self.current_ml < self.goal_ml
            && self.last_drink > 0
            && now.saturating_sub(self.last_drink) >= interval_seconds
    }
}

pub fn format_thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separator_matches_examples() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(250), "250");
        assert_eq!(format_thousands(1250), "1,250");
        assert_eq!(format_thousands(2_000), "2,000");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn add_and_undo_track_the_last_glass() {
        let mut water = WaterTracker::new(0, 2000, 0);
        assert!(!water.undo_available());
        water.add(250, 100);
        water.add(150, 200);
        assert_eq!(water.current_ml(), 400);
        assert_eq!(water.last_drink(), 200);
        assert!(water.undo());
        assert_eq!(water.current_ml(), 250);
        assert!(!water.undo());
        assert_eq!(water.current_ml(), 250);
    }

    #[test]
    fn progress_and_percent_are_clamped() {
        let mut water = WaterTracker::new(1000, 2000, 0);
        assert_eq!(water.percent_text(), "50%");
        assert_eq!(water.summary_text(), "1,000 / 2,000 ml");
        water.add(5000, 10);
        assert_eq!(water.progress(), 1.0);
        assert_eq!(water.percent_text(), "100%");
    }

    #[test]
    fn goal_can_be_changed_from_settings() {
        let mut water = WaterTracker::new(1000, 2000, 0);
        water.set_goal_ml(2500);
        assert_eq!(water.goal_ml(), 2500);
        assert_eq!(water.summary_text(), "1,000 / 2,500 ml");
        water.set_goal_ml(0);
        assert_eq!(water.goal_ml(), 1);
    }

    #[test]
    fn nudge_waits_for_the_interval() {
        let water = WaterTracker::new(500, 2000, 1_000);
        assert!(!water.needs_nudge(1_000 + 60 * 60, NUDGE_AFTER_SECONDS));
        assert!(water.needs_nudge(1_000 + NUDGE_AFTER_SECONDS, NUDGE_AFTER_SECONDS));

        let fresh = WaterTracker::new(0, 2000, 0);
        assert!(!fresh.needs_nudge(i64::MAX / 2, NUDGE_AFTER_SECONDS));
    }
}
