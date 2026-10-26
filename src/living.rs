//! The living mascot model: breathing pulse, ferrofluid cursor pull, slow
//! corona rotation, and the five-bar audio response used by Closed mode.
//!
//! All values are computed in logical pixels so they can be applied straight
//! to Slint properties without rescaling.

use std::f32::consts::TAU;

/// The PRD asks for a 0.25 Hz breathing pulse.
pub const BREATH_HZ: f32 = 0.25;

const PULL_MAX: f32 = 4.2;
const PULL_LAMBDA: f32 = 140.0;
const PULL_RANGE: f32 = 320.0;

/// Audio is considered playing after this much sustained signal.
const ACTIVE_AFTER: f32 = 0.25;
/// And considered stopped after this much silence (covers quiet passages).
const INACTIVE_AFTER: f32 = 1.6;
/// Peak amplitude that counts as real playback. Real desktops report a small
/// non-zero noise floor on the default endpoint (measured up to ~0.017 here),
/// so a threshold near that value never settles back to Idle by itself.
/// 0.03 is roughly -30 dBFS: below that audio is effectively inaudible.
const AUDIO_THRESHOLD: f32 = 0.03;

#[derive(Debug, Clone, Copy)]
pub struct LivingFrame {
    pub orb_scale: f32,
    pub orb_glow: f32,
    pub aura_scale: f32,
    pub aura_opacity: f32,
    pub corona_angle: f32,
    pub orb_offset_x: f32,
    pub orb_offset_y: f32,
    pub wave: [f32; 5],
    pub media_active: bool,
}

impl Default for LivingFrame {
    fn default() -> Self {
        Self {
            orb_scale: 1.0,
            orb_glow: 0.48,
            aura_scale: 1.0,
            aura_opacity: 0.48,
            corona_angle: 0.0,
            orb_offset_x: 0.0,
            orb_offset_y: 0.0,
            wave: [0.0; 5],
            media_active: false,
        }
    }
}

pub struct Living {
    elapsed: f32,
    offset_x: f32,
    offset_y: f32,
    corona_angle: f32,
    smoothed_peak: f32,
    loud_time: f32,
    quiet_time: f32,
    media_active: bool,
    wave: [f32; 5],
}

impl Default for Living {
    fn default() -> Self {
        Self::new()
    }
}

impl Living {
    pub fn new() -> Self {
        Self {
            elapsed: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            corona_angle: 0.0,
            smoothed_peak: 0.0,
            loud_time: 0.0,
            quiet_time: 0.0,
            media_active: false,
            wave: [0.0; 5],
        }
    }

    pub fn tick(
        &mut self,
        dt: f32,
        cursor: Option<(f32, f32)>,
        peak: f32,
        playing: bool,
        bands: [f32; 5],
    ) -> LivingFrame {
        let dt = dt.clamp(0.001, 0.1);
        self.elapsed = (self.elapsed + dt) % 3600.0;

        self.smoothed_peak = self.smoothed_peak.max(peak.clamp(0.0, 1.0));
        self.smoothed_peak *= (-dt * 2.2).exp();

        // A live SMTC session is authoritative even when the track is quiet;
        // the endpoint peak keeps games and other non-SMTC audio working.
        if self.smoothed_peak > AUDIO_THRESHOLD || playing {
            self.loud_time += dt;
            self.quiet_time = 0.0;
        } else {
            self.quiet_time += dt;
            self.loud_time = 0.0;
        }
        if !self.media_active && self.loud_time >= ACTIVE_AFTER {
            self.media_active = true;
        }
        if self.media_active && self.quiet_time >= INACTIVE_AFTER {
            self.media_active = false;
        }

        let breath = (self.elapsed * TAU * BREATH_HZ).sin();
        // The aura trails the core slightly, which makes the plasma feel liquid
        // instead of like three images pulsing in lockstep.
        let trailing = (self.elapsed * TAU * BREATH_HZ - 0.55).sin();

        let (target_x, target_y) = self.pull_target(cursor);
        let follow = 1.0 - (-dt * 9.0).exp();
        self.offset_x += (target_x - self.offset_x) * follow;
        self.offset_y += (target_y - self.offset_y) * follow;

        self.corona_angle = (self.corona_angle + dt * 9.0) % 360.0;

        for (index, bar) in self.wave.iter_mut().enumerate() {
            let target = if self.media_active {
                bands[index].clamp(0.0, 1.0)
            } else {
                0.0
            };
            *bar += (target - *bar) * (1.0 - (-dt * 18.0).exp());
        }

        LivingFrame {
            orb_scale: 1.0 + 0.035 * breath,
            orb_glow: 0.48 + 0.14 * breath,
            aura_scale: 1.0 + 0.06 * trailing,
            aura_opacity: 0.44 + 0.10 * trailing,
            corona_angle: self.corona_angle,
            orb_offset_x: self.offset_x,
            orb_offset_y: self.offset_y,
            wave: self.wave,
            media_active: self.media_active,
        }
    }

    fn pull_target(&self, cursor: Option<(f32, f32)>) -> (f32, f32) {
        let Some((x, y)) = cursor else {
            return (0.0, 0.0);
        };
        let distance = (x * x + y * y).sqrt();
        if distance < 0.5 || distance > PULL_RANGE {
            return (0.0, 0.0);
        }

        // delta p = u * Lmax * (1 - e^(-d/lambda)), exactly as specified in
        // the PRD, with a soft fade at the sensing edge so it glides home.
        let magnitude = PULL_MAX * (1.0 - (-distance / PULL_LAMBDA).exp());
        let fade = 1.0 - smoothstep(PULL_RANGE * 0.8, PULL_RANGE, distance);
        (
            x / distance * magnitude * fade,
            y / distance * magnitude * fade,
        )
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breathing_stays_within_spec() {
        let mut living = Living::new();
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for _ in 0..240 {
            let frame = living.tick(1.0 / 60.0, None, 0.0, false, [0.0; 5]);
            min = min.min(frame.orb_scale);
            max = max.max(frame.orb_scale);
        }
        assert!(min < 0.98, "breathing should contract, got {min}");
        assert!(max > 1.02, "breathing should expand, got {max}");
    }

    #[test]
    fn ferrofluid_pull_is_clamped() {
        let mut living = Living::new();
        let mut frame = LivingFrame::default();
        for _ in 0..120 {
            frame = living.tick(1.0 / 60.0, Some((250.0, 0.0)), 0.0, false, [0.0; 5]);
        }
        assert!(frame.orb_offset_x > 3.0);
        assert!(frame.orb_offset_x <= PULL_MAX + 0.01);
        assert!(frame.orb_offset_y.abs() < 0.01);
    }

    #[test]
    fn media_activity_uses_hysteresis() {
        let mut living = Living::new();
        let mut frame = LivingFrame::default();
        for _ in 0..20 {
            frame = living.tick(1.0 / 30.0, None, 0.20, false, [0.0; 5]);
        }
        assert!(frame.media_active, "loud audio should activate Closed");

        for _ in 0..30 {
            frame = living.tick(1.0 / 30.0, None, 0.0, false, [0.0; 5]);
        }
        assert!(
            frame.media_active,
            "a short quiet passage must not drop Closed"
        );

        for _ in 0..60 {
            frame = living.tick(1.0 / 30.0, None, 0.0, false, [0.0; 5]);
        }
        assert!(!frame.media_active, "sustained silence should return Idle");
    }

    #[test]
    fn smtc_playback_activates_without_endpoint_peak() {
        let mut living = Living::new();
        let mut frame = LivingFrame::default();
        for _ in 0..20 {
            frame = living.tick(1.0 / 30.0, None, 0.0, true, [0.0; 5]);
        }
        assert!(frame.media_active, "a playing session should open Closed");

        for _ in 0..60 {
            frame = living.tick(1.0 / 30.0, None, 0.0, false, [0.0; 5]);
        }
        assert!(!frame.media_active, "a paused session should return Idle");
    }

    #[test]
    fn wave_bars_follow_the_spectrum_and_decay() {
        let mut living = Living::new();
        let bands = [0.9, 0.7, 0.5, 0.3, 0.1];
        for _ in 0..30 {
            living.tick(1.0 / 30.0, None, 0.0, true, bands);
        }
        let active = living.tick(1.0 / 30.0, None, 0.0, true, bands);
        assert!(active.wave[0] > active.wave[4], "bands keep their shape");
        assert!(active.wave[0] > 0.5);

        for _ in 0..120 {
            living.tick(1.0 / 30.0, None, 0.0, false, bands);
        }
        let silent = living.tick(1.0 / 30.0, None, 0.0, false, bands);
        assert!(silent.wave.iter().all(|bar| *bar < 0.01));
    }
}
