//! Errors, logging and running commands with a timeout.

use std::fmt;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A tool failure. `actionable` ones are the agent's to fix (wrong text, no
/// device) and are reported with a hint; the rest are plain errors.
#[derive(Debug)]
pub struct Error {
    pub actionable: bool,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn actionable(message: impl Into<String>) -> Self {
        Error {
            actionable: true,
            message: message.into(),
        }
    }
    pub fn other(message: impl Into<String>) -> Self {
        Error {
            actionable: false,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::other(e.to_string())
    }
}

/// Bails out with an actionable error.
#[macro_export]
macro_rules! actionable {
    ($($arg:tt)*) => { return Err($crate::util::Error::actionable(format!($($arg)*))) };
}

/// Writes to stderr (the MCP client's log) and to $LOG_FILE when set.
pub fn log(message: &str) {
    eprintln!("{message}");
    if let Some(file) = std::env::var_os("LOG_FILE")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
    {
        let _ = writeln!(f, "[{}] {message}", unix_secs());
    }
}

pub fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Runs `cmd` to the end, killing it after `timeout`, and returns its stdout.
/// Fails on a non-zero exit unless `allow_fail`.
pub fn run(mut cmd: Command, timeout: Duration, allow_fail: bool) -> Result<Vec<u8>> {
    let what = format!("{:?}", cmd.get_program());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::other(format!("{what}: {e}")))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out_thread = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_thread = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::other(format!(
                "{what} timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    if !status.success() && !allow_fail {
        let text = String::from_utf8_lossy(&stderr);
        let text = if text.trim().is_empty() {
            String::from_utf8_lossy(&stdout)
        } else {
            text
        };
        return Err(Error::other(format!(
            "{what} failed ({status}): {}",
            text.trim()
        )));
    }
    Ok(stdout)
}

/// The user's home folder.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Lowercase hex sha256 of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Truncates to `max` characters, ending with an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max - 1).collect();
    t.push('…');
    t
}

/// Whether a process runs. A child this process started and that exited is
/// a zombie until reaped, which kill(pid, 0) would still call alive.
pub fn alive(pid: i32) -> bool {
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == pid {
        return false;
    }
    // signal 0 only checks that it exists (EPERM: someone else's, but there)
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// A TCP port free on loopback right now.
pub fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decodes standard base64, skipping padding and whitespace.
pub fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let Some(v) = B64.iter().position(|&b| b == c) else {
            continue;
        };
        buf = buf << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for (raw, enc) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw), enc);
            assert_eq!(base64_decode(enc), raw);
        }
        assert_eq!(base64_decode("Zm8=\n"), b"fo");
    }

    #[test]
    fn this_process_is_alive() {
        assert!(alive(std::process::id() as i32));
    }
}
