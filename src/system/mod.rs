pub mod battery;
pub mod volume;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;
use crate::system::battery::get_power_info;
use crate::system::volume::{start_volume_listener, VolumeInfo};

pub struct SystemHudManager;

impl SystemHudManager {
    pub fn start(weak_app: slint::Weak<crate::AppWindow>) {
        let (volume_tx, volume_rx) = channel::<VolumeInfo>();
        start_volume_listener(volume_tx);

        let hud_counter = Arc::new(AtomicU32::new(0));

        // Auto-dismiss countdown timer thread (runs every 100ms)
        let weak_app_timer = weak_app.clone();
        let counter_timer = hud_counter.clone();
        std::thread::Builder::new()
            .name("kripton-hud-dismiss-timer".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    let val = counter_timer.load(Ordering::Relaxed);
                    if val > 0 {
                        let new_val = val.saturating_sub(1);
                        counter_timer.store(new_val, Ordering::Relaxed);
                        if new_val == 0 {
                            let _ = weak_app_timer.upgrade_in_event_loop(|app| {
                                app.set_is_hud_active(false);
                            });
                        }
                    }
                }
            })
            .expect("Failed to spawn hud dismiss timer");

        // Worker thread to poll volume events and power status
        let counter_worker = hud_counter;
        std::thread::Builder::new()
            .name("kripton-system-hud-worker".into())
            .spawn(move || {
                let mut last_power = get_power_info();
                let mut last_volume: Option<VolumeInfo> = None;
                let mut initial_skip = true;

                loop {
                    // Check volume events (up to ~1200ms per loop)
                    let start = std::time::Instant::now();
                    while start.elapsed() < Duration::from_millis(1200) {
                        if let Ok(vol) = volume_rx.recv_timeout(Duration::from_millis(200)) {
                            // Check if volume actually changed from last known
                            let is_changed = match last_volume {
                                Some(lv) => (lv.level - vol.level).abs() > 0.005 || lv.is_muted != vol.is_muted,
                                None => true,
                            };
                            last_volume = Some(vol);

                            if is_changed {
                                let is_muted = vol.is_muted;
                                let level = vol.level;
                                let icon = if is_muted {
                                    "🔇"
                                } else if level < 0.33 {
                                    "🔈"
                                } else if level < 0.66 {
                                    "🔉"
                                } else {
                                    "🔊"
                                };
                                let title = if is_muted {
                                    "Muted".to_string()
                                } else {
                                    format!("{:.0}%", level * 100.0)
                                };

                                // Reset timer to 20 ticks = 2.0 seconds
                                counter_worker.store(20, Ordering::Relaxed);

                                let _ = weak_app.upgrade_in_event_loop(move |app| {
                                    app.set_hud_icon(icon.into());
                                    app.set_hud_title(title.into());
                                    app.set_hud_subtitle("Master Volume".into());
                                    app.set_hud_progress(level);
                                    app.set_hud_show_bar(true);
                                    app.set_hud_bar_color(slint::Color::from_rgb_u8(29, 185, 84));
                                    app.set_is_hud_active(true);
                                });
                            }
                        }
                    }

                    // Check power / battery status
                    if let Some(curr) = get_power_info() {
                        if !initial_skip {
                            if let Some(prev) = &last_power {
                                if curr.is_plugged != prev.is_plugged {
                                    if curr.is_plugged {
                                        let pct = curr.percent;
                                        counter_worker.store(25, Ordering::Relaxed); // 2.5s
                                        let _ = weak_app.upgrade_in_event_loop(move |app| {
                                            app.set_hud_icon("⚡".into());
                                            app.set_hud_title(format!("{}% Charging", pct).into());
                                            app.set_hud_subtitle("Charger Connected".into());
                                            app.set_hud_progress(pct as f32 / 100.0);
                                            app.set_hud_show_bar(true);
                                            app.set_hud_bar_color(slint::Color::from_rgb_u8(34, 197, 94));
                                            app.set_is_hud_active(true);
                                        });
                                    } else {
                                        let pct = curr.percent;
                                        counter_worker.store(25, Ordering::Relaxed); // 2.5s
                                        let _ = weak_app.upgrade_in_event_loop(move |app| {
                                            app.set_hud_icon("🔋".into());
                                            app.set_hud_title(format!("{}% On Battery", pct).into());
                                            app.set_hud_subtitle("Charger Disconnected".into());
                                            app.set_hud_progress(pct as f32 / 100.0);
                                            app.set_hud_show_bar(true);
                                            app.set_hud_bar_color(slint::Color::from_rgb_u8(234, 179, 8));
                                            app.set_is_hud_active(true);
                                        });
                                    }
                                } else if curr.has_battery && curr.percent <= 20 && prev.percent > 20 && !curr.is_plugged {
                                    let pct = curr.percent;
                                    counter_worker.store(35, Ordering::Relaxed); // 3.5s
                                    let _ = weak_app.upgrade_in_event_loop(move |app| {
                                        app.set_hud_icon("🪫".into());
                                        app.set_hud_title(format!("Low Battery ({}%)", pct).into());
                                        app.set_hud_subtitle("Plug in charger".into());
                                        app.set_hud_progress(pct as f32 / 100.0);
                                        app.set_hud_show_bar(true);
                                        app.set_hud_bar_color(slint::Color::from_rgb_u8(239, 68, 68));
                                        app.set_is_hud_active(true);
                                    });
                                }
                            }
                        }
                        last_power = Some(curr);
                        initial_skip = false;
                    }
                }
            })
            .expect("Failed to spawn system hud worker");
    }
}
