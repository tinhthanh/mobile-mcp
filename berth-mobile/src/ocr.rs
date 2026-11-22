//! Apple Vision OCR (macOS): the helper is built into this binary and
//! unpacked, checked, into the cache the first time it is needed.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crate::util::{log, run, sha256_hex};

#[cfg(has_ocr)]
const OCR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ocr"));
#[cfg(not(has_ocr))]
const OCR: &[u8] = &[];

static BINARY: OnceLock<Option<PathBuf>> = OnceLock::new();

fn unpack() -> Option<PathBuf> {
    if OCR.is_empty() || std::env::var_os("MOBILEMCP_DISABLE_OCR").is_some() {
        return None;
    }
    let sum = sha256_hex(OCR);
    let dir = crate::paths::cache_dir();
    let path = dir.join(format!("ocr-{}", &sum[..16]));
    let ok = std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == sum);
    if !ok {
        use std::os::unix::fs::PermissionsExt;
        crate::paths::ensure_private(&dir).ok()?;
        let tmp = dir.join(format!("ocr.{}.tmp", std::process::id()));
        std::fs::write(&tmp, OCR).ok()?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700)).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(path)
}

/// "text|x,y" lines in device pixels (each starting with a space, the plain
/// text mark), or None when OCR isn't available here.
pub fn ocr_lines(png: &[u8]) -> Option<Vec<String>> {
    let bin = BINARY.get_or_init(unpack).as_ref()?;
    let dir = std::env::temp_dir().join(format!("berth-mobile-ocr-{}", std::process::id()));
    crate::paths::ensure_private(&dir).ok()?;
    let file = dir.join("screen.png");
    let result = (|| {
        std::fs::write(&file, png).ok()?;
        let mut cmd = Command::new(bin);
        cmd.arg(&file);
        match run(cmd, Duration::from_secs(30), false) {
            Ok(out) => Some(
                String::from_utf8_lossy(&out)
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| format!(" {l}"))
                    .collect(),
            ),
            Err(e) => {
                log(&format!("ocr failed: {e}"));
                None
            }
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}
