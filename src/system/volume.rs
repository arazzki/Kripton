use std::sync::mpsc::Sender;
use windows_core::implement;
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator,
};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeInfo {
    pub level: f32, // 0.0 to 1.0
    pub is_muted: bool,
}

#[implement(IAudioEndpointVolumeCallback)]
pub struct VolumeCallback {
    sender: Sender<VolumeInfo>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows_core::Result<()> {
        if !pnotify.is_null() {
            unsafe {
                let data = &*pnotify;
                let info = VolumeInfo {
                    level: data.fMasterVolume,
                    is_muted: data.bMuted.as_bool(),
                };
                let _ = self.sender.send(info);
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub fn get_current_volume() -> windows_core::Result<VolumeInfo> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
        let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;

        let level = endpoint_volume.GetMasterVolumeLevelScalar()?;
        let muted = endpoint_volume.GetMute()?;

        Ok(VolumeInfo {
            level,
            is_muted: muted.as_bool(),
        })
    }
}

pub fn start_volume_listener(sender: Sender<VolumeInfo>) {
    std::thread::Builder::new()
        .name("kripton-volume-watcher".into())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(err) => {
                    eprintln!("[Kripton] Failed to create MMDeviceEnumerator: {:?}", err);
                    return;
                }
            };

            let device: IMMDevice = match enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) {
                Ok(d) => d,
                Err(err) => {
                    eprintln!("[Kripton] Failed to get default audio endpoint: {:?}", err);
                    return;
                }
            };

            let endpoint_volume: IAudioEndpointVolume = match device.Activate(CLSCTX_ALL, None) {
                Ok(v) => v,
                Err(err) => {
                    eprintln!("[Kripton] Failed to activate IAudioEndpointVolume: {:?}", err);
                    return;
                }
            };

            let cb = VolumeCallback { sender };
            let callback: IAudioEndpointVolumeCallback = cb.into();
            if let Err(err) = endpoint_volume.RegisterControlChangeNotify(&callback) {
                eprintln!("[Kripton] RegisterControlChangeNotify failed: {:?}", err);
                return;
            }

            // Keep the thread alive so COM callback registration remains active
            loop {
                std::thread::park();
            }
        })
        .expect("Failed to spawn volume watcher thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume() {
        let vol = get_current_volume();
        println!("Current Windows Master Volume: {:?}", vol);
        assert!(vol.is_ok());
    }
}
