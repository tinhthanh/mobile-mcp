//! Downloads pinned files. The SHA-256 of everything berth-mobile fetches is
//! compiled into the binary, and the binary is what the user allowed in
//! Shipyard, so a fetched file is covered by the same consent.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use sha2::Digest;

use crate::util::{Error, Result, hex, log};

const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

/// A file fetched from `url` and checked against `sha256`.
pub struct Pinned {
    pub url: &'static str,
    pub sha256: &'static str,
}

impl Pinned {
    pub fn name(&self) -> &'static str {
        self.url.rsplit('/').next().unwrap_or("download")
    }
}

/// Returns `dir/<name>`, downloading it first unless a copy that matches the
/// pin is there already.
pub fn cached(pin: &Pinned, dir: &Path) -> Result<std::path::PathBuf> {
    let dest = dir.join(pin.name());
    if dest.is_file() && file_sha256(&dest)? == pin.sha256 {
        return Ok(dest);
    }
    crate::paths::ensure_private(dir)?;
    download(pin, &dest)?;
    Ok(dest)
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Downloads to `dest`, refusing (and deleting) a file whose hash differs.
/// Plain http only from loopback, for local test builds.
pub fn download(pin: &Pinned, dest: &Path) -> Result<()> {
    if !(pin.url.starts_with("https://") || pin.url.starts_with("http://127.0.0.1")) {
        return Err(Error::other(format!(
            "refusing to fetch {} over plain http",
            pin.url
        )));
    }
    log(&format!("fetching {}", pin.url));
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .build(),
    );
    let resp = agent
        .get(pin.url)
        .call()
        .map_err(|e| Error::other(format!("{}: {e}", pin.url)))?;
    let mut reader = resp
        .into_body()
        .into_with_config()
        .limit(MAX_DOWNLOAD)
        .reader();
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::other(format!("{}: {e}", pin.url)))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n])?;
    }
    f.sync_all()?;
    drop(f);
    let got = hex(&h.finalize());
    if got != pin.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(Error::other(format!(
            "{} doesn't match its pinned SHA-256 (got {got})",
            pin.name()
        )));
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}
