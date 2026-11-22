//! Several agents can drive one phone at once, each with its own berth-mobile
//! process. A per-device file lock keeps their actions from interleaving, and
//! per-process lease files let a process tell whether another one still uses
//! the phone's UiAutomator2 server before stopping it.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crate::util::{Error, Result};

/// How long a tool call waits for another agent to finish with the phone.
const LOCK_WAIT: Duration = Duration::from_secs(120);

fn safe(serial: &str) -> String {
    serial
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn device_dir(serial: &str) -> Result<PathBuf> {
    let d = crate::paths::run_dir().join(safe(serial));
    crate::paths::ensure_private(&d)?;
    Ok(d)
}

/// Held while a tool call uses the device; released on drop.
pub struct DeviceLock {
    _file: File,
}

pub fn lock_device(serial: &str) -> Result<DeviceLock> {
    let path = device_dir(serial)?.join("lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(DeviceLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(Error::actionable(format!(
                        "{serial} is busy: another agent has been using it for {}s. Try again later",
                        LOCK_WAIT.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

fn lease_path(serial: &str, pid: u32) -> Result<PathBuf> {
    Ok(device_dir(serial)?.join(format!("uia2.{pid}")))
}

/// Records that this process just used the device's UiAutomator2 server.
pub fn touch_lease(serial: &str) {
    if let Ok(p) = lease_path(serial, std::process::id()) {
        let _ = std::fs::write(p, b"");
    }
}

pub fn drop_lease(serial: &str) {
    if let Ok(p) = lease_path(serial, std::process::id()) {
        let _ = std::fs::remove_file(p);
    }
}

/// Whether another live process used the server within `within`. Leases of
/// processes that are gone are removed.
pub fn others_using(serial: &str, within: Duration) -> bool {
    let Ok(dir) = device_dir(serial) else {
        return false;
    };
    let me = std::process::id();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    let now = SystemTime::now();
    let mut busy = false;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix("uia2.")
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        if !crate::util::alive(pid as i32) {
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        let recent = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age < within);
        busy |= recent;
    }
    busy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_across_handles() {
        let dir = std::env::temp_dir().join(format!("berth-mobile-lock-{}", std::process::id()));
        unsafe { std::env::set_var("BERTH_MOBILE_HOME", &dir) };
        let a = lock_device("serial:1").unwrap();
        let path = crate::paths::run_dir().join("serial_1").join("lock");
        let f = File::options().write(true).open(&path).unwrap();
        assert!(matches!(
            f.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(a);
        assert!(f.try_lock().is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
