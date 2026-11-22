//! Where berth-mobile keeps things. Never in the plugin's own folder: Shipyard
//! copies that folder to boxes as it is, and replaces it on every update.

use std::path::PathBuf;

use crate::util::home;

/// Durable state: fetched adb, PCAPdroid api keys, locks.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BERTH_MOBILE_HOME") {
        return PathBuf::from(d);
    }
    if cfg!(target_os = "macos") {
        return home().join("Library/Application Support/berth-mobile");
    }
    match std::env::var_os("XDG_STATE_HOME") {
        Some(d) => PathBuf::from(d).join("berth-mobile"),
        None => home().join(".local/state/berth-mobile"),
    }
}

/// Re-creatable files: downloaded APKs, the unpacked OCR helper.
pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BERTH_MOBILE_CACHE") {
        return PathBuf::from(d);
    }
    if cfg!(target_os = "macos") {
        return home().join("Library/Caches/berth-mobile");
    }
    match std::env::var_os("XDG_CACHE_HOME") {
        Some(d) => PathBuf::from(d).join("berth-mobile"),
        None => home().join(".cache/berth-mobile"),
    }
}

/// Lock and lease files shared by every berth-mobile process on this computer.
pub fn run_dir() -> PathBuf {
    state_dir().join("run")
}

/// Creates `dir` (and parents) readable by this user only.
pub fn ensure_private(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}
