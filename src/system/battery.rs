use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

#[derive(Debug, Clone, PartialEq)]
pub struct PowerInfo {
    pub is_plugged: bool,
    pub is_charging: bool,
    pub percent: u8,
    pub has_battery: bool,
}

pub fn get_power_info() -> Option<PowerInfo> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe {
        if GetSystemPowerStatus(&mut status).is_ok() {
            let has_battery = status.BatteryFlag != 128 && status.BatteryLifePercent != 255;
            let is_plugged = status.ACLineStatus == 1;
            let is_charging = (status.BatteryFlag & 8) != 0 || (is_plugged && status.BatteryLifePercent < 100);
            let percent = if status.BatteryLifePercent <= 100 {
                status.BatteryLifePercent
            } else {
                100
            };

            Some(PowerInfo {
                is_plugged,
                is_charging,
                percent,
                has_battery,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_status() {
        let info = get_power_info();
        println!("Power info on current PC: {:?}", info);
    }
}
