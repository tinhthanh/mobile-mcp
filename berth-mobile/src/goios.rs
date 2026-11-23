//! go-ios (MIT): pairing info, the iOS 17+ tunnel, installing apps, the
//! developer disk image, running WebDriverAgent and forwarding its port.
//! Fetched at run time against a hash compiled into this binary.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::fetch::Pinned;
use crate::util::{Error, Result, log, run, sleep_ms};

pub const GO_IOS: Pinned = Pinned {
    url: "https://github.com/danielpaulus/go-ios/releases/download/v1.3.2/go-ios-mac.zip",
    sha256: "100f225bfdd039081bcbdaf45029df3e67032673f3e0ca1fa3c050898c230c57",
};

/// iOS 17+ personalized developer disk image (from Xcode, re-hosted by
/// go-ios's maintainers; source github.com/doronz88/DeveloperDiskImage).
/// The phone mounts it only with a signature Apple's TSS server gives for
/// these exact digests.
pub const DDI: Pinned = Pinned {
    url: "https://deviceboxhq.com/ddi-17E5179g.zip",
    sha256: "b74e5d038df88fcf954f2a503bcbeb94a3fb78ad508a557665ac6bf325bece1a",
};

const TIMEOUT: Duration = Duration::from_secs(60);

fn dir() -> PathBuf {
    crate::paths::state_dir().join("tools").join("go-ios-1.3.2")
}

/// The `ios` binary, fetched and unpacked once.
pub fn binary() -> Result<PathBuf> {
    let bin = dir().join("ios");
    if bin.is_file() {
        return Ok(bin);
    }
    let cache = crate::paths::cache_dir().join("tools");
    let zip = crate::fetch::cached(&GO_IOS, &cache)?;
    let staging = dir().with_extension("new");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut cmd = Command::new("/usr/bin/ditto");
    cmd.arg("-x").arg("-k").arg(&zip).arg(&staging);
    run(cmd, Duration::from_secs(60), false)?;
    if !staging.join("ios").is_file() {
        return Err(Error::other("go-ios archive has no ios binary"));
    }
    let _ = std::fs::remove_dir_all(dir());
    std::fs::rename(&staging, dir())?;
    log("fetched go-ios 1.3.2");
    Ok(bin)
}

/// Whether go-ios is here already (status must not download anything).
pub fn present() -> bool {
    dir().join("ios").is_file()
}

fn command(udid: Option<&str>, args: &[&str]) -> Result<Command> {
    let mut cmd = Command::new(binary()?);
    cmd.args(args);
    if let Some(u) = udid {
        cmd.arg(format!("--udid={u}"));
    }
    Ok(cmd)
}

/// Runs go-ios and returns its stdout (its log goes to stderr).
pub fn ios(udid: Option<&str>, args: &[&str]) -> Result<String> {
    let out = run(command(udid, args)?, TIMEOUT, false)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// How many iPhones/iPads macOS sees on USB (Apple vendor id), whether or
/// not go-ios or signing is set up: enough to say what is missing.
pub fn usb_count() -> usize {
    let mut cmd = Command::new("/usr/sbin/ioreg");
    cmd.args(["-p", "IOUSB", "-l", "-w0"]);
    let out = run(cmd, Duration::from_secs(10), false)
        .map(|o| String::from_utf8_lossy(&o).into_owned())
        .unwrap_or_default();
    out.lines()
        .filter(|l| {
            l.contains("\"USB Product Name\"")
                && (l.contains("\"iPhone\"") || l.contains("\"iPad\""))
        })
        .count()
}

/// UDIDs of the iPhones attached over USB.
pub fn list() -> Result<Vec<String>> {
    let out = ios(None, &["list"])?;
    let v: Value = serde_json::from_str(out.trim()).unwrap_or(Value::Null);
    Ok(v.get("deviceList")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// (ProductType, ProductVersion), e.g. ("iPhone14,4", "26.6.2").
pub fn product(udid: &str) -> Option<(String, String)> {
    let out = ios(Some(udid), &["info"]).ok()?;
    let v: Value = serde_json::from_str(out.lines().last()?.trim()).ok()?;
    Some((
        v.get("ProductType")?.as_str()?.to_string(),
        v.get("ProductVersion")?.as_str()?.to_string(),
    ))
}

/// Bundle ids of installed apps (`all`: system apps too).
pub fn apps(udid: &str, all: bool) -> Result<Vec<String>> {
    let args: &[&str] = if all {
        &["apps", "--all", "--list"]
    } else {
        &["apps", "--list"]
    };
    Ok(ios(Some(udid), args)?
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(String::from))
        .collect())
}

pub fn install(udid: &str, app: &Path) -> Result<()> {
    let p = app.to_string_lossy();
    run(
        command(Some(udid), &["install", &format!("--path={p}")])?,
        Duration::from_secs(300),
        false,
    )?;
    Ok(())
}

/// Mounts the developer disk image unless one is mounted (needed for XCTest).
pub fn ensure_image(udid: &str) -> Result<()> {
    let base = crate::paths::cache_dir().join("ddi");
    crate::fetch::cached(&DDI, &base)?;
    let b = base.to_string_lossy();
    // mounts, or says it is mounted already
    run(
        command(Some(udid), &["image", "auto", &format!("--basedir={b}")])?,
        Duration::from_secs(120),
        false,
    )?;
    Ok(())
}

/// Starts the shared userspace tunnel (no root) unless it runs. It stays up
/// for every process and device, like the adb server.
pub fn ensure_tunnel(udid: &str) -> Result<()> {
    let up = |u: &str| ios(None, &["tunnel", "ls"]).is_ok_and(|o| o.contains(u));
    if up(udid) {
        return Ok(());
    }
    let log_file = crate::paths::run_dir().join("tunnel.log");
    crate::paths::ensure_private(&crate::paths::run_dir())?;
    let out = std::fs::File::create(&log_file)?;
    let mut cmd = command(None, &["tunnel", "start", "--userspace"])?;
    detach(&mut cmd);
    let child = cmd
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()?;
    // remembered so the last user can stop it (stop_tunnel_if_unused)
    std::fs::write(tunnel_pid_file(), child.id().to_string())?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if up(udid) {
            return Ok(());
        }
        sleep_ms(500);
    }
    Err(Error::other(format!(
        "go-ios tunnel didn't reach the iPhone (see {})",
        log_file.display()
    )))
}

fn tunnel_pid_file() -> PathBuf {
    crate::paths::run_dir().join("tunnel.pid")
}

/// Stops the tunnel berth-mobile started once no iPhone runs WebDriverAgent
/// any more. A tunnel someone else started (no pid file) is left alone.
pub fn stop_tunnel_if_unused() {
    let Ok(entries) = std::fs::read_dir(crate::paths::run_dir()) else {
        return;
    };
    let pid_in = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
    };
    let wda_running = entries
        .flatten()
        .filter_map(|e| pid_in(e.path().join("wda.pid")))
        .any(crate::util::alive);
    if wda_running {
        return;
    }
    if let Some(pid) = pid_in(tunnel_pid_file()) {
        if crate::util::alive(pid) {
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            log("stopped the go-ios tunnel: no iPhone in use");
        }
        let _ = std::fs::remove_file(tunnel_pid_file());
    }
}

/// Own process group: survives this process, and a client's Ctrl-C.
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// Starts WebDriverAgent bound to the phone's loopback only (USE_IP), so it
/// is reachable over the cable and not from the Wi-Fi network. Returns its pid.
pub fn spawn_wda(udid: &str, bundle: &str, log_file: &Path) -> Result<u32> {
    let out = std::fs::File::create(log_file)?;
    let mut cmd = command(
        Some(udid),
        &[
            "runwda",
            &format!("--bundleid={bundle}"),
            &format!("--testrunnerbundleid={bundle}"),
            "--xctestconfig=WebDriverAgentRunner.xctest",
            "--env=USE_IP=127.0.0.1",
        ],
    )?;
    detach(&mut cmd);
    let child = cmd
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()?;
    Ok(child.id())
}

/// Forwards localhost:`port` to port 8100 on the phone, for this process.
pub fn spawn_forward(udid: &str, port: u16) -> Result<Child> {
    let mut cmd = command(Some(udid), &["forward", &port.to_string(), "8100"])?;
    Ok(cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?)
}
