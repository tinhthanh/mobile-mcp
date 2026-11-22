//! adb: which binary, and the device commands the tools use.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use crate::actionable;
use crate::fetch::Pinned;
use crate::util::{Error, Result, log, run, sleep_ms};

/// The platform-tools build every berth tool uses, the same one vglink pins:
/// two adb clients of different versions keep killing each other's server.
pub const ADB_VERSION: &str = "37.0.1";

#[cfg(target_os = "macos")]
pub const PLATFORM_TOOLS: Option<Pinned> = Some(Pinned {
    url: "https://dl.google.com/android/repository/platform-tools_r37.0.1-darwin.zip",
    sha256: "ee39ad5967e95c2a07f04dbcbde96b1a0c916ba376096db5d2f498b7727a5d1d",
});
#[cfg(not(target_os = "macos"))]
pub const PLATFORM_TOOLS: Option<Pinned> = None;

const TIMEOUT: Duration = Duration::from_secs(30);

static ADB: Mutex<Option<PathBuf>> = Mutex::new(None);

fn tools_dir() -> PathBuf {
    crate::paths::state_dir().join("tools")
}

fn fetched_adb() -> PathBuf {
    tools_dir().join("platform-tools").join("adb")
}

/// adb binaries on this computer, best first.
fn candidates() -> Vec<PathBuf> {
    let mut out = vec![fetched_adb()];
    if let Some(h) =
        std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
    {
        out.push(PathBuf::from(h).join("platform-tools/adb"));
    }
    let home = crate::util::home();
    out.push(home.join("Library/Android/sdk/platform-tools/adb"));
    out.push(home.join("Android/Sdk/platform-tools/adb"));
    out.push(PathBuf::from("/opt/homebrew/bin/adb"));
    out.push(PathBuf::from("/usr/local/bin/adb"));
    out.push(PathBuf::from("/usr/bin/adb"));
    if let Some(path) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join("adb")));
    }
    out.dedup();
    out.into_iter().filter(|p| p.is_file()).collect()
}

/// The version an adb binary reports ("37.0.1"), if it runs.
pub fn adb_version(adb: &Path) -> Option<String> {
    let mut cmd = Command::new(adb);
    cmd.arg("version");
    let out = run(cmd, Duration::from_secs(5), false).ok()?;
    let text = String::from_utf8_lossy(&out).into_owned();
    text.lines()
        .find_map(|l| l.strip_prefix("Version "))
        .map(|v| v.split('-').next().unwrap_or(v).trim().to_string())
}

/// Finds adb: $BERTH_MOBILE_ADB, else one of the pinned version, else (when
/// `fetch`) Google's pinned platform-tools, else any adb there is.
pub fn find_adb(fetch: bool) -> Option<PathBuf> {
    if let Some(p) =
        std::env::var_os("BERTH_MOBILE_ADB").or_else(|| std::env::var_os("MOBILEMCP_ADB"))
    {
        return Some(PathBuf::from(p));
    }
    let all = candidates();
    if let Some(p) = all
        .iter()
        .find(|p| adb_version(p).as_deref() == Some(ADB_VERSION))
    {
        return Some(p.clone());
    }
    if fetch && PLATFORM_TOOLS.is_some() {
        match fetch_platform_tools() {
            Ok(p) => return Some(p),
            Err(e) => log(&format!("couldn't fetch platform-tools: {e}")),
        }
    }
    all.into_iter().next()
}

/// Downloads Google's platform-tools into the state folder and unpacks them
/// whole (adb needs the files beside it).
pub fn fetch_platform_tools() -> Result<PathBuf> {
    let Some(pin) = PLATFORM_TOOLS else {
        return Err(Error::other(
            "install adb with this computer's package manager",
        ));
    };
    let dir = tools_dir();
    crate::paths::ensure_private(&dir)?;
    let zip = dir.join("platform-tools.zip");
    crate::fetch::download(&pin, &zip)?;
    let staging = dir.join("platform-tools.new");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut cmd = Command::new("/usr/bin/ditto");
    cmd.arg("-x").arg("-k").arg(&zip).arg(&staging);
    let unpacked = run(cmd, Duration::from_secs(120), false);
    let _ = std::fs::remove_file(&zip);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(dir.join("platform-tools"));
    std::fs::rename(staging.join("platform-tools"), dir.join("platform-tools"))?;
    let _ = std::fs::remove_dir_all(&staging);
    log("fetched Google's platform-tools");
    Ok(fetched_adb())
}

/// The adb this process uses, found once.
pub fn adb_path() -> Result<PathBuf> {
    let mut g = ADB.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = g.as_ref() {
        return Ok(p.clone());
    }
    let p = find_adb(true).ok_or_else(|| {
        Error::actionable("adb not found. Run the mobile plugin's setup in Shipyard (Settings → Plugins), or install Android platform-tools")
    })?;
    *g = Some(p.clone());
    Ok(p)
}

/// Runs adb with `args` (no device selected).
pub fn adb_global(args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new(adb_path()?);
    cmd.args(args);
    run(cmd, TIMEOUT, false)
}

/// A line of `adb devices`.
pub struct Listed {
    pub serial: String,
    pub state: String,
}

pub fn list_devices() -> Result<Vec<Listed>> {
    let out = adb_global(&["devices"])?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut it = l.trim().split('\t');
            let serial = it.next()?.trim();
            let state = it.next()?.trim();
            (!serial.is_empty()).then(|| Listed {
                serial: serial.into(),
                state: state.into(),
            })
        })
        .collect())
}

/// Serials of devices that are online and authorized.
pub fn ready_devices() -> Result<Vec<String>> {
    Ok(list_devices()?
        .into_iter()
        .filter(|d| d.state == "device")
        .map(|d| d.serial)
        .collect())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn parse(s: &str) -> Result<Direction> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "up" => Direction::Up,
            "down" => Direction::Down,
            "left" => Direction::Left,
            "right" => Direction::Right,
            _ => actionable!("Swipe direction \"{s}\" is not supported"),
        })
    }
}

pub const BUTTONS: [&str; 10] = [
    "BACK",
    "HOME",
    "ENTER",
    "VOLUME_UP",
    "VOLUME_DOWN",
    "DPAD_CENTER",
    "DPAD_UP",
    "DPAD_DOWN",
    "DPAD_LEFT",
    "DPAD_RIGHT",
];

pub fn validate_package(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        actionable!("Invalid package name: \"{name}\"");
    }
    Ok(())
}

/// Escapes text for `input text` through the device shell.
pub fn escape_shell_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\'\"` \t\n\r|&;()<>{}[]$*?#~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// One Android device.
pub struct Device {
    pub serial: String,
}

impl Device {
    pub fn new(serial: &str) -> Self {
        Device {
            serial: serial.to_string(),
        }
    }

    pub fn adb(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut cmd = Command::new(adb_path()?);
        cmd.arg("-s").arg(&self.serial).args(args);
        run(cmd, TIMEOUT, false)
    }

    pub fn adb_text(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.adb(args)?).replace('\r', ""))
    }

    pub fn shell(&self, args: &[&str]) -> Result<String> {
        let mut all = vec!["shell"];
        all.extend_from_slice(args);
        self.adb_text(&all)
    }

    pub fn getprop(&self, name: &str) -> String {
        self.shell(&["getprop", name])
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// "model android-version" details for the devices tool.
    pub fn name(&self) -> String {
        let avd = self.getprop("ro.boot.qemu.avd_name");
        if !avd.is_empty() {
            return avd.replace('_', " ");
        }
        let model = self.getprop("ro.product.model");
        if model.is_empty() {
            self.serial.clone()
        } else {
            model
        }
    }

    pub fn has_package(&self, package: &str) -> Result<bool> {
        let out = self.shell(&["pm", "list", "packages", package])?;
        Ok(out
            .lines()
            .any(|l| l.trim() == format!("package:{package}")))
    }

    pub fn version_name(&self, package: &str) -> Option<String> {
        let out = self.shell(&["dumpsys", "package", package]).ok()?;
        out.lines().find_map(|l| {
            l.trim()
                .strip_prefix("versionName=")
                .map(|v| v.trim().to_string())
        })
    }

    pub fn screen_size(&self) -> Result<(i64, i64)> {
        let out = self.shell(&["wm", "size"])?;
        // "Physical size: 720x1600" (and "Override size: ..." after it when set)
        let last = out.lines().rfind(|l| l.contains("size:")).unwrap_or("");
        let dims = last.rsplit(' ').next().unwrap_or("");
        let mut it = dims.trim().split('x').map(|n| n.parse::<i64>());
        match (it.next(), it.next()) {
            (Some(Ok(w)), Some(Ok(h))) => Ok((w, h)),
            _ => Err(Error::other("Failed to get screen size")),
        }
    }

    pub fn list_apps(&self) -> Result<Vec<String>> {
        let out = self.shell(&[
            "cmd",
            "package",
            "query-activities",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
        ])?;
        let mut apps: Vec<String> = Vec::new();
        for l in out.lines() {
            if let Some(p) = l.trim().strip_prefix("packageName=")
                && !apps.iter().any(|a| a == p)
            {
                apps.push(p.to_string());
            }
        }
        Ok(apps)
    }

    fn launcher_activity(&self, package: &str) -> Option<String> {
        let out = self
            .shell(&[
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "-c",
                "android.intent.category.LAUNCHER",
                package,
            ])
            .ok()?;
        let last = out
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())?
            .to_string();
        // the component goes through the device shell: accept a plain component name only
        let plain = last.split_once('/').is_some_and(|(p, c)| {
            !p.is_empty()
                && !c.is_empty()
                && p.chars()
                    .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.')
                && c.chars()
                    .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == '$')
        });
        (last.starts_with(&format!("{package}/")) && plain).then_some(last)
    }

    pub fn launch_app(&self, package: &str) -> Result<()> {
        validate_package(package)?;
        // am start takes ~0.3 s where monkey takes ~3 s on low-end devices
        if let Some(activity) = self.launcher_activity(package) {
            // "$" in nested activity classes would expand in the device shell
            let escaped = activity.replace('$', "\\$");
            if let Ok(out) = self.shell(&["am", "start", "-n", &escaped])
                && !out.contains("Error")
            {
                return Ok(());
            }
        }
        match self.shell(&[
            "monkey",
            "-p",
            package,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ]) {
            Ok(_) => Ok(()),
            Err(_) => actionable!(
                "Failed launching app with package name \"{package}\", please make sure it exists"
            ),
        }
    }

    pub fn terminate_app(&self, package: &str) -> Result<()> {
        validate_package(package)?;
        self.shell(&["am", "force-stop", package])?;
        Ok(())
    }

    fn input_swipe(&self, x0: i64, y0: i64, x1: i64, y1: i64, ms: i64) -> Result<()> {
        self.shell(&[
            "input",
            "swipe",
            &x0.to_string(),
            &y0.to_string(),
            &x1.to_string(),
            &y1.to_string(),
            &ms.to_string(),
        ])?;
        Ok(())
    }

    pub fn swipe(&self, d: Direction) -> Result<()> {
        let (w, h) = self.screen_size()?;
        let cx = w >> 1;
        let f = |v: i64, p: f64| (v as f64 * p).floor() as i64;
        let (x0, y0, x1, y1) = match d {
            Direction::Up => (cx, f(h, 0.8), cx, f(h, 0.2)),
            Direction::Down => (cx, f(h, 0.2), cx, f(h, 0.8)),
            Direction::Left => (f(w, 0.8), f(h, 0.5), f(w, 0.2), f(h, 0.5)),
            Direction::Right => (f(w, 0.2), f(h, 0.5), f(w, 0.8), f(h, 0.5)),
        };
        self.input_swipe(x0, y0, x1, y1, 1000)
    }

    pub fn swipe_from(&self, x: i64, y: i64, d: Direction, distance: Option<i64>) -> Result<()> {
        let (w, h) = self.screen_size()?;
        let dy = distance
            .filter(|&v| v != 0)
            .unwrap_or((h as f64 * 0.3).floor() as i64);
        let dx = distance
            .filter(|&v| v != 0)
            .unwrap_or((w as f64 * 0.3).floor() as i64);
        let (x1, y1) = match d {
            Direction::Up => (x, (y - dy).max(0)),
            Direction::Down => (x, (y + dy).min(h)),
            Direction::Left => ((x - dx).max(0), y),
            Direction::Right => ((x + dx).min(w), y),
        };
        self.input_swipe(x, y, x1, y1, 1000)
    }

    pub fn tap(&self, x: i64, y: i64) -> Result<()> {
        self.shell(&["input", "tap", &x.to_string(), &y.to_string()])?;
        Ok(())
    }

    pub fn long_press(&self, x: i64, y: i64, ms: i64) -> Result<()> {
        self.input_swipe(x, y, x, y, ms)
    }

    pub fn double_tap(&self, x: i64, y: i64) -> Result<()> {
        self.tap(x, y)?;
        sleep_ms(100);
        self.tap(x, y)
    }

    pub fn press_button(&self, button: &str) -> Result<()> {
        let b = button.to_ascii_uppercase();
        if !BUTTONS.contains(&b.as_str()) {
            actionable!("Button \"{button}\" is not supported");
        }
        self.shell(&["input", "keyevent", &format!("KEYCODE_{b}")])?;
        Ok(())
    }

    /// Types through `input text`, which only takes ASCII and appends.
    pub fn send_keys(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if !text.is_ascii() {
            actionable!("Typing non-ASCII text needs UiAutomator2 on the phone; it isn't running");
        }
        self.shell(&["input", "text", &escape_shell_text(text)])?;
        Ok(())
    }

    fn display_count(&self) -> usize {
        self.shell(&["dumpsys", "SurfaceFlinger", "--display-id"])
            .map(|o| o.lines().filter(|l| l.starts_with("Display ")).count())
            .unwrap_or(1)
    }

    fn first_display_id(&self) -> Option<String> {
        if let Ok(out) = self.shell(&["cmd", "display", "get-displays"]) {
            for l in out.lines() {
                if l.starts_with("Display id ")
                    && l.contains(", state ON,")
                    && let Some(rest) = l.split("uniqueId \"").nth(1)
                {
                    let id = rest.split('"').next().unwrap_or("");
                    return Some(id.strip_prefix("local:").unwrap_or(id).to_string());
                }
            }
        }
        let dump = self.shell(&["dumpsys", "display"]).ok()?;
        let re = regex_lite::Regex::new(
            r"DisplayViewport\{type=INTERNAL[^}]*isActive=true[^}]*uniqueId='([^']+)'",
        )
        .ok()?;
        if let Some(c) = re.captures(&dump) {
            let id = &c[1];
            return Some(id.strip_prefix("local:").unwrap_or(id).to_string());
        }
        let re = regex_lite::Regex::new(r"(?s)Display Id=(\d+).*?Display State=ON").ok()?;
        re.captures(&dump).map(|c| c[1].to_string())
    }

    /// The screen as PNG bytes; on a phone with several displays, the one that is on.
    pub fn screenshot(&self) -> Result<Vec<u8>> {
        if self.display_count() > 1
            && let Some(id) = self.first_display_id()
        {
            return self.adb(&["exec-out", "screencap", "-p", "-d", &id]);
        }
        self.adb(&["exec-out", "screencap", "-p"])
    }

    /// `uiautomator dump`: slow, but needs nothing installed.
    pub fn uiautomator_dump(&self) -> Result<String> {
        for _ in 0..10 {
            let mut dump = String::from_utf8_lossy(&self.adb(&[
                "exec-out",
                "uiautomator",
                "dump",
                "/dev/tty",
            ])?)
            .into_owned();
            if !dump.contains("<?xml") && !dump.contains("null root node") {
                // some vendor builds (e.g. ZTE) print nothing to /dev/tty: go through a file
                let _ = self.shell(&["uiautomator", "dump", "/data/local/tmp/ui.xml"]);
                dump = String::from_utf8_lossy(&self.adb(&[
                    "exec-out",
                    "cat",
                    "/data/local/tmp/ui.xml",
                ])?)
                .into_owned();
            }
            if let Some(i) = dump.find("<?xml") {
                return Ok(dump[i..].to_string());
            }
            // "null root node returned by UiTestAutomationBridge" while the ui settles
        }
        actionable!("Failed to get UIAutomator XML")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_shell_specials() {
        assert_eq!(escape_shell_text("a b;c"), "a\\ b\\;c");
        assert_eq!(escape_shell_text("#x ~y $(z)"), "\\#x\\ \\~y\\ \\$\\(z\\)");
        assert_eq!(escape_shell_text("it's"), "it\\'s");
    }

    #[test]
    fn validates_package_names() {
        assert!(validate_package("com.example.app").is_ok());
        assert!(validate_package("com.some-company.app").is_ok());
        assert!(validate_package("a;rm -rf").is_err());
        assert!(validate_package("").is_err());
    }
}
