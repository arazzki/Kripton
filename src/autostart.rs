//! Sprint 6: per-user autostart through the Windows `Run` key.
//!
//! Only `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` is touched, so no
//! administrator rights are required and uninstalling is a single value delete.

use std::{io, os::windows::ffi::OsStrExt, path::Path};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
            HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
        },
    },
};

pub const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
pub const VALUE_NAME: PCWSTR = w!("Kripton");

/// `true` when the current user has any Kripton value in the Run key.
pub fn is_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_QUERY_VALUE, &mut key);
        if status != ERROR_SUCCESS {
            return false;
        }

        let mut size = 0u32;
        let status = RegQueryValueExW(key, VALUE_NAME, None, None, None, Some(&mut size));
        let _ = RegCloseKey(key);
        status == ERROR_SUCCESS && size > 0
    }
}

/// Enable or disable autostart for the current executable.
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    if enabled {
        let exe = std::env::current_exe()?;
        set_command(&command_line(&exe))
    } else {
        remove()
    }
}

fn set_command(command: &str) -> io::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key);
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }

        let wide: Vec<u16> = std::ffi::OsStr::new(command)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let bytes = std::slice::from_raw_parts(
            wide.as_ptr() as *const u8,
            wide.len() * std::mem::size_of::<u16>(),
        );
        let status = RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }
    }
    Ok(())
}

fn remove() -> io::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key);
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }

        let status = RegDeleteValueW(key, VALUE_NAME);
        let _ = RegCloseKey(key);
        if status != ERROR_SUCCESS && status != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }
    }
    Ok(())
}

/// The Run value must quote the executable so paths with spaces still work.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        assert_eq!(
            command_line(Path::new(r"C:\Program Files\Kripton\kripton.exe")),
            r#""C:\Program Files\Kripton\kripton.exe""#
        );
    }
}
