//! Win32 probes for battery state and the live audio peak.
//!
//! Sprint 1 only needs a scalar peak from the default render endpoint so the
//! notch can tell whether media is playing. Sprint 2 replaces the scalar with
//! a WASAPI loopback FFT for the five-band visualizer.

use std::time::{Duration, Instant};

use windows::Win32::{
    Media::Audio::{
        eConsole, eRender, Endpoints::IAudioMeterInformation, IMMDeviceEnumerator,
        MMDeviceEnumerator,
    },
    System::{
        Com::{CoCreateInstance, CLSCTX_ALL},
        Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
    },
};

const BATTERY_FLAG_CHARGING: u8 = 8;
const BATTERY_FLAG_NO_BATTERY: u8 = 128;
const BATTERY_FLAG_UNKNOWN: u8 = 255;
const BATTERY_PERCENT_UNKNOWN: u8 = 255;

/// Radius of the stroked path that forms the closed-mode battery ring. The
/// ring is 18 px wide; a 2.5 px stroke leaves a 7.75 px centre radius.
pub const RING_CENTER: f32 = 9.0;
pub const RING_RADIUS: f32 = 7.75;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryStatus {
    pub present: bool,
    /// 0.0 - 100.0. Unknown or absent batteries report 100.0 so the desktop
    /// fallback can draw a full track plus an AC glyph.
    pub percent: f32,
    pub charging: bool,
    pub on_ac: bool,
}

impl BatteryStatus {
    pub const fn desktop() -> Self {
        Self {
            present: false,
            percent: 100.0,
            charging: false,
            on_ac: false,
        }
    }
}

impl Default for BatteryStatus {
    fn default() -> Self {
        Self::desktop()
    }
}

pub fn read_battery() -> BatteryStatus {
    let mut raw = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut raw) }.is_err() {
        return BatteryStatus::desktop();
    }

    let no_battery = raw.BatteryFlag & BATTERY_FLAG_NO_BATTERY != 0;
    let unknown = raw.BatteryFlag == BATTERY_FLAG_UNKNOWN
        || raw.BatteryLifePercent == BATTERY_PERCENT_UNKNOWN;
    let present = !no_battery && !unknown;
    let percent = if raw.BatteryLifePercent <= 100 {
        raw.BatteryLifePercent as f32
    } else {
        0.0
    };

    BatteryStatus {
        present,
        percent: if present { percent } else { 100.0 },
        charging: raw.BatteryFlag & BATTERY_FLAG_CHARGING != 0,
        on_ac: raw.ACLineStatus == 1,
    }
}

/// End point and flags for the progress arc. Coordinates are in the 18 x 18
/// viewbox used by the Slint `Path`, starting at 12 o'clock and moving
/// clockwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryArc {
    pub end_x: f32,
    pub end_y: f32,
    pub large: bool,
    pub visible: bool,
    pub full: bool,
}

pub fn battery_arc(percent: f32) -> BatteryArc {
    let progress = (percent / 100.0).clamp(0.0, 1.0);
    let angle = progress * std::f32::consts::TAU;

    BatteryArc {
        end_x: RING_CENTER + RING_RADIUS * angle.sin(),
        end_y: RING_CENTER - RING_RADIUS * angle.cos(),
        large: progress > 0.5,
        visible: progress > 0.004,
        full: progress > 0.996,
    }
}

/// Thin wrapper around `IAudioMeterInformation` that reconnects when the
/// default output device changes or is unplugged.
pub struct AudioMeter {
    meter: Option<IAudioMeterInformation>,
    last_attempt: Instant,
    logged_failure: bool,
}

impl AudioMeter {
    pub fn new() -> Self {
        let mut meter = Self {
            meter: None,
            last_attempt: Instant::now(),
            logged_failure: false,
        };
        meter.connect();
        meter
    }

    /// Latest peak amplitude (0.0 - 1.0) from the default render endpoint.
    pub fn peak(&mut self) -> f32 {
        if let Some(meter) = &self.meter {
            match unsafe { meter.GetPeakValue() } {
                Ok(value) => return value.clamp(0.0, 1.0),
                Err(error) => {
                    eprintln!("[Kripton] audio meter lost: {error}");
                    self.meter = None;
                    self.last_attempt = Instant::now();
                }
            }
        }

        if self.last_attempt.elapsed() >= Duration::from_secs(2) {
            self.connect();
            if let Some(meter) = &self.meter {
                if let Ok(value) = unsafe { meter.GetPeakValue() } {
                    return value.clamp(0.0, 1.0);
                }
            }
        }
        0.0
    }

    fn connect(&mut self) {
        self.last_attempt = Instant::now();
        match activate_meter() {
            Ok(meter) => {
                self.meter = Some(meter);
                self.logged_failure = false;
            }
            Err(error) => {
                if !self.logged_failure {
                    eprintln!("[Kripton] audio meter unavailable: {error}");
                    self.logged_failure = true;
                }
            }
        }
    }
}

fn activate_meter() -> windows::core::Result<IAudioMeterInformation> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        device.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_starts_at_twelve_oclock() {
        let arc = battery_arc(0.0);
        assert!(!arc.visible);
        assert!((arc.end_x - 9.0).abs() < 0.001);
        assert!((arc.end_y - 1.25).abs() < 0.001);
    }

    #[test]
    fn arc_tracks_quarter_turns() {
        let quarter = battery_arc(25.0);
        assert!((quarter.end_x - 16.75).abs() < 0.01);
        assert!((quarter.end_y - 9.0).abs() < 0.01);
        assert!(!quarter.large);

        let half = battery_arc(50.0);
        assert!((half.end_x - 9.0).abs() < 0.01);
        assert!((half.end_y - 16.75).abs() < 0.01);
        assert!(!half.large);

        let three_quarters = battery_arc(75.0);
        assert!((three_quarters.end_x - 1.25).abs() < 0.01);
        assert!((three_quarters.end_y - 9.0).abs() < 0.01);
        assert!(three_quarters.large);
    }

    #[test]
    fn full_battery_uses_full_circle() {
        let full = battery_arc(100.0);
        assert!(full.visible);
        assert!(full.full);
        assert!((full.end_x - 9.0).abs() < 0.01);
        assert!((full.end_y - 1.25).abs() < 0.01);
    }

    #[test]
    fn desktop_fallback_is_not_a_battery() {
        let status = BatteryStatus::desktop();
        assert!(!status.present);
        assert_eq!(status.percent, 100.0);
    }
}
