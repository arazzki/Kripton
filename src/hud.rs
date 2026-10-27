//! Sprint 6: transient hardware HUDs shown inside the notch.
//!
//! The state machine is deliberately pure so the 3 second lifetime and the
//! charging thresholds can be unit tested without COM, Win32 or Slint.

use std::time::{Duration, Instant};

use crate::state::NotchMode;

pub const HUD_DURATION: Duration = Duration::from_secs(3);
pub const HEADPHONE_SIZE: (f32, f32) = (320.0, 64.0);
pub const CHARGING_SIZE: (f32, f32) = (300.0, 58.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudKind {
    Headphone,
    Charging,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudContent {
    pub kind: HudKind,
    pub title: String,
    pub status: String,
    pub connected: bool,
    pub percent: u8,
    pub charging: bool,
}

impl HudContent {
    pub fn headphone(name: impl Into<String>, connected: bool) -> Self {
        let name = name.into();
        let title = if name.trim().is_empty() {
            "Headphones".to_string()
        } else {
            name
        };
        Self {
            kind: HudKind::Headphone,
            title,
            status: if connected {
                "Connected".to_string()
            } else {
                "Disconnected".to_string()
            },
            connected,
            percent: 0,
            charging: false,
        }
    }

    pub fn charging(
        title: impl Into<String>,
        status: impl Into<String>,
        percent: u8,
        charging: bool,
    ) -> Self {
        Self {
            kind: HudKind::Charging,
            title: title.into(),
            status: status.into(),
            connected: true,
            percent,
            charging,
        }
    }
}

/// Keeps the active HUD and the mode that must be restored after 3 seconds.
pub struct HudState {
    current: Option<HudContent>,
    shown_at: Instant,
    return_mode: NotchMode,
}

impl Default for HudState {
    fn default() -> Self {
        Self::new()
    }
}

impl HudState {
    pub fn new() -> Self {
        Self {
            current: None,
            shown_at: Instant::now(),
            return_mode: NotchMode::Stealth,
        }
    }

    /// Show or replace the current HUD. Replacing keeps the original return
    /// mode and restarts the 3 second timer.
    pub fn show(&mut self, content: HudContent, now: Instant, return_mode: NotchMode) {
        if self.current.is_none() {
            self.return_mode = return_mode;
        }
        self.current = Some(content);
        self.shown_at = now;
    }

    pub fn active(&self) -> bool {
        self.current.is_some()
    }

    /// Clear the HUD when the user explicitly changed the mode before the
    /// timer elapsed, so the timer cannot override the user's action.
    pub fn dismiss(&mut self) {
        self.current = None;
    }

    /// Returns the mode to restore once the HUD has been visible for 3 s.
    pub fn expire(&mut self, now: Instant) -> Option<NotchMode> {
        if self.current.is_some() && now.duration_since(self.shown_at) >= HUD_DURATION {
            self.current = None;
            Some(self.return_mode)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSnapshot {
    pub on_ac: bool,
    pub charging: bool,
    pub percent: u8,
}

impl PowerSnapshot {
    pub fn from_battery(status: &crate::system::BatteryStatus) -> Self {
        Self {
            on_ac: status.on_ac,
            charging: status.charging,
            percent: status.percent.round().clamp(0.0, 100.0) as u8,
        }
    }
}

/// Returns a charging HUD only on a meaningful power transition. Polling the
/// same state every 500 ms must not re-trigger the HUD.
pub fn charging_hud_event(previous: PowerSnapshot, next: PowerSnapshot) -> Option<HudContent> {
    let plugged = next.on_ac && !previous.on_ac;
    let unplugged = !next.on_ac && previous.on_ac;
    let charging_changed = next.charging != previous.charging;
    let low = !next.on_ac && next.percent <= 20 && previous.percent > 20;
    let critical = !next.on_ac && next.percent <= 10 && previous.percent > 10;

    if !(plugged || unplugged || charging_changed || low || critical) {
        return None;
    }

    let (title, status) = if next.on_ac && (next.charging || next.percent >= 99) {
        if next.percent >= 99 {
            ("Fully Charged", "Power adapter connected")
        } else {
            ("Charging", "Power adapter connected")
        }
    } else if next.on_ac {
        ("Plugged In", "Power adapter connected")
    } else if next.percent <= 20 {
        ("Battery Low", "Connect a charger")
    } else {
        ("On Battery", "Power adapter disconnected")
    };

    Some(HudContent::charging(
        title,
        status,
        next.percent,
        next.charging,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_sizes_match_the_prd() {
        assert_eq!(HEADPHONE_SIZE, (320.0, 64.0));
        assert_eq!(CHARGING_SIZE, (300.0, 58.0));
        assert_eq!(HUD_DURATION, Duration::from_secs(3));
    }

    #[test]
    fn headphone_hud_expires_after_three_seconds() {
        let start = Instant::now();
        let mut hud = HudState::new();
        hud.show(
            HudContent::headphone("Sony WH-1000XM5", true),
            start,
            NotchMode::Closed,
        );

        assert!(hud.active());
        assert_eq!(hud.current.as_ref().unwrap().title, "Sony WH-1000XM5");
        assert_eq!(hud.current.as_ref().unwrap().status, "Connected");
        assert!(hud.expire(start + Duration::from_millis(2_999)).is_none());
        assert_eq!(hud.expire(start + HUD_DURATION), Some(NotchMode::Closed));
        assert!(!hud.active());
    }

    #[test]
    fn refreshing_a_hud_extends_the_timer_and_keeps_return_mode() {
        let start = Instant::now();
        let mut hud = HudState::new();
        hud.show(
            HudContent::headphone("AirPods Pro", true),
            start,
            NotchMode::Expanded,
        );
        hud.show(
            HudContent::headphone("AirPods Pro", false),
            start + Duration::from_secs(2),
            NotchMode::Stealth,
        );

        assert!(hud.expire(start + Duration::from_millis(4_999)).is_none());
        assert_eq!(
            hud.expire(start + Duration::from_secs(5)),
            Some(NotchMode::Expanded)
        );
    }

    #[test]
    fn dismiss_prevents_the_timer_from_overriding_the_user() {
        let start = Instant::now();
        let mut hud = HudState::new();
        hud.show(
            HudContent::headphone("Headset", true),
            start,
            NotchMode::Closed,
        );
        hud.dismiss();
        assert!(hud.expire(start + HUD_DURATION).is_none());
    }

    #[test]
    fn charging_hud_fires_on_plug_and_unplug() {
        let battery = PowerSnapshot {
            on_ac: false,
            charging: false,
            percent: 64,
        };
        let plugged = PowerSnapshot {
            on_ac: true,
            charging: true,
            ..battery
        };
        let event = charging_hud_event(battery, plugged).unwrap();
        assert_eq!(event.kind, HudKind::Charging);
        assert_eq!(event.title, "Charging");
        assert_eq!(event.percent, 64);

        let unplugged = charging_hud_event(plugged, battery).unwrap();
        assert_eq!(unplugged.title, "On Battery");
        assert_eq!(unplugged.status, "Power adapter disconnected");
    }

    #[test]
    fn charging_hud_does_not_repeat_for_a_steady_state() {
        let state = PowerSnapshot {
            on_ac: true,
            charging: true,
            percent: 80,
        };
        assert!(charging_hud_event(state, state).is_none());
    }

    #[test]
    fn charging_hud_fires_once_at_the_low_threshold() {
        let above = PowerSnapshot {
            on_ac: false,
            charging: false,
            percent: 21,
        };
        let low = PowerSnapshot {
            percent: 20,
            ..above
        };
        let below = PowerSnapshot {
            percent: 19,
            ..above
        };

        assert_eq!(charging_hud_event(above, low).unwrap().title, "Battery Low");
        assert!(charging_hud_event(low, below).is_none());
    }

    #[test]
    fn fully_charged_state_is_reported() {
        let previous = PowerSnapshot {
            on_ac: true,
            charging: true,
            percent: 98,
        };
        let next = PowerSnapshot {
            charging: false,
            percent: 100,
            ..previous
        };
        let event = charging_hud_event(previous, next).unwrap();
        assert_eq!(event.title, "Fully Charged");
        assert_eq!(event.percent, 100);
    }
}
