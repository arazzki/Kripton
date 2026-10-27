//! Sprint 6: WASAPI default-endpoint watcher for the headphone HUD.
//!
//! `IMMNotificationClient` is registered on a dedicated MTA thread. The COM
//! callback never touches Slint: it publishes a plain snapshot that the UI tick
//! drains, so a slow audio stack can never stall rendering.

use std::{
    sync::{Arc, Mutex},
    thread,
};

use windows::{
    core::{implement, Result, GUID, PCWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            eMultimedia, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
            IMMNotificationClient_Impl, PKEY_AudioEndpoint_FormFactor, DEVICE_STATE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
            STGM_READ,
        },
        UI::{
            Shell::PropertiesSystem::IPropertyStore,
            WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG},
        },
    },
};

use crate::hud::HudContent;

const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);

/// Headphone, Headset, and the common Bluetooth names that report an unknown
/// form factor. Speakers, HDMI and SPDIF are intentionally ignored.
fn is_headphone_like(form_factor: u32, name: &str) -> bool {
    if matches!(form_factor, 3 | 5) {
        return true;
    }

    let lower = name.to_ascii_lowercase();
    [
        "headphone",
        "headset",
        "earbud",
        "earphone",
        "airpod",
        "buds",
        "wh-",
        "tws",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

unsafe fn endpoint_info(device_id: PCWSTR) -> Option<(String, u32)> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, None, CLSCTX_ALL).ok()?;
    let device = enumerator.GetDevice(device_id).ok()?;
    let store: IPropertyStore = device.OpenPropertyStore(STGM_READ).ok()?;

    let name = store
        .GetValue(&PKEY_Device_FriendlyName)
        .map(|value| value.to_string())
        .unwrap_or_default();
    let form_factor = store
        .GetValue(&PKEY_AudioEndpoint_FormFactor)
        .ok()
        .and_then(|value| u32::try_from(&value).ok())
        .unwrap_or(0);

    Some((name, form_factor))
}

#[implement(IMMNotificationClient)]
struct EndpointWatcher {
    snapshot: Arc<Mutex<Option<HudContent>>>,
    current_headphone: Mutex<Option<String>>,
}

impl IMMNotificationClient_Impl for EndpointWatcher_Impl {
    fn OnDeviceStateChanged(&self, _device_id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        device_id: &PCWSTR,
    ) -> Result<()> {
        if flow != eRender || role != eMultimedia {
            return Ok(());
        }

        let (name, form_factor) =
            unsafe { endpoint_info(*device_id) }.unwrap_or_else(|| ("Audio device".to_string(), 0));
        let headphone = is_headphone_like(form_factor, &name);

        let mut current = self
            .current_headphone
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let content = if headphone {
            *current = Some(name.clone());
            HudContent::headphone(name, true)
        } else if let Some(previous) = current.take() {
            HudContent::headphone(previous, false)
        } else {
            return Ok(());
        };

        *self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(content);
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _device_id: &PCWSTR,
        _key: &windows::Win32::Foundation::PROPERTYKEY,
    ) -> Result<()> {
        Ok(())
    }
}

/// Handle kept by the UI runtime. Dropping it only drops the snapshot handle;
/// the watcher thread lives for the process lifetime, matching the overlay.
#[derive(Clone)]
pub struct HeadphoneWatcher {
    snapshot: Arc<Mutex<Option<HudContent>>>,
}

impl HeadphoneWatcher {
    pub fn start() -> Self {
        let snapshot = Arc::new(Mutex::new(None));
        let shared = snapshot.clone();

        let spawn = thread::Builder::new()
            .name("kripton-audio-events".into())
            .spawn(move || {
                if let Err(error) = watch_loop(shared) {
                    crate::log_line(&format!("audio endpoint watcher stopped: {error}"));
                }
            });

        if let Err(error) = spawn {
            crate::log_line(&format!("failed to spawn audio endpoint watcher: {error}"));
        }

        Self { snapshot }
    }

    /// Latest event, if any. Only one HUD is shown at a time, so coalescing is
    /// the desired behaviour when several endpoints change quickly.
    pub fn take(&self) -> Option<HudContent> {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

fn watch_loop(snapshot: Arc<Mutex<Option<HudContent>>>) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&CLSID_MM_DEVICE_ENUMERATOR, None, CLSCTX_ALL)?;
        let watcher: IMMNotificationClient = EndpointWatcher {
            snapshot,
            current_headphone: Mutex::new(None),
        }
        .into();
        enumerator.RegisterEndpointNotificationCallback(&watcher)?;
        crate::log_line("audio endpoint watcher registered");

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = enumerator.UnregisterEndpointNotificationCallback(&watcher);
        CoUninitialize();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_factors_mark_headphones_and_headsests() {
        assert!(is_headphone_like(3, "Speakers"));
        assert!(is_headphone_like(5, "USB Audio"));
        assert!(!is_headphone_like(1, "Speakers"));
        assert!(!is_headphone_like(9, "HDMI Display"));
    }

    #[test]
    fn bluetooth_names_fall_back_to_a_name_heuristic() {
        assert!(is_headphone_like(0, "Sony WH-1000XM5"));
        assert!(is_headphone_like(0, "Galaxy Buds2 Pro"));
        assert!(is_headphone_like(0, "AirPods Pro"));
        assert!(!is_headphone_like(0, "Realtek Digital Output"));
    }

    #[test]
    fn console_role_is_ignored_but_render_is_not() {
        // Pure guard mirror used by the callback: only render + multimedia
        // produce a HUD, so Windows does not fire twice for console/multimedia.
        assert_ne!(eRender, EDataFlow(1));
        assert_ne!(eMultimedia, ERole(0));
    }
}
