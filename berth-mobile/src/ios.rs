//! An iPhone, driven through WebDriverAgent (Appium's, BSD), re-signed with
//! the person's team and run with go-ios. WebDriverAgent is bound to the
//! phone's loopback and reached over the cable; it runs only while agents use
//! the phone and is stopped when no process has used it for a while.

use std::process::Child;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::adb::Direction;
use crate::util::{Error, Result, log, sleep_ms};
use crate::{goios, lock, signing};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(60);
pub const IDLE_STOP: Duration = Duration::from_secs(5 * 60);

pub struct IosPhone {
    pub udid: String,
    port: u16,
    forward: Option<Child>,
    session: Option<String>,
    size: Option<(f64, f64)>,
    pub last_use: Option<Instant>,
    agent: ureq::Agent,
}

fn pid_file(udid: &str) -> std::path::PathBuf {
    crate::paths::run_dir().join(udid).join("wda.pid")
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn b64decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

impl IosPhone {
    pub fn new(udid: &str) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(REQUEST_TIMEOUT))
                .http_status_as_error(false)
                .build(),
        );
        IosPhone {
            udid: udid.to_string(),
            port: 0,
            forward: None,
            session: None,
            size: None,
            last_use: None,
            agent,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = self.url(path);
        let err = |e: ureq::Error| Error::other(format!("wda {method} {path}: {e}"));
        let resp = match (method, body) {
            ("GET", _) => self.agent.get(&url).call().map_err(err)?,
            ("DELETE", _) => self.agent.delete(&url).call().map_err(err)?,
            (_, Some(b)) => self.agent.post(&url).send_json(b).map_err(err)?,
            (_, None) => self.agent.post(&url).send_json(json!({})).map_err(err)?,
        };
        let status = resp.status();
        let v: Value = resp
            .into_body()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_json()
            .map_err(|e| Error::other(format!("wda {method} {path}: {e}")))?;
        let value = v.get("value").cloned().unwrap_or(Value::Null);
        if !status.is_success() || value.get("error").is_some() {
            let msg = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            if msg.contains("could not be, unlocked")
                || msg.contains("BSErrorCodeDescription=Locked")
            {
                return Err(Error::actionable(
                    "The iPhone is locked: ask the person to unlock it (berth-mobile never enters the passcode)",
                ));
            }
            return Err(Error::other(format!(
                "wda {method} {path}: {}",
                msg.lines().next().unwrap_or(msg)
            )));
        }
        Ok(v)
    }

    fn ready(&self) -> bool {
        self.port != 0
            && self
                .request("GET", "/status", None)
                .is_ok_and(|v| v.pointer("/value/ready") == Some(&Value::Bool(true)))
    }

    /// Everything WebDriverAgent needs, then it running and reachable.
    fn ensure(&mut self) -> Result<()> {
        self.last_use = Some(Instant::now());
        lock::touch_lease(&self.udid);
        if self.ready() {
            return Ok(());
        }
        // forward first: another process may have started WebDriverAgent already
        if self.forward.is_none() {
            goios::ensure_tunnel(&self.udid)?;
            self.port = std::net::TcpListener::bind("127.0.0.1:0")?
                .local_addr()?
                .port();
            self.forward = Some(goios::spawn_forward(&self.udid, self.port)?);
            sleep_ms(500);
            if self.ready() {
                return Ok(());
            }
        }
        let profile = signing::read_profile()?;
        signing::check_device(&profile, &self.udid)?;
        let bundle = profile.runner_bundle();
        let pid_path = pid_file(&self.udid);
        let running = std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .is_some_and(alive);
        if !running {
            goios::ensure_tunnel(&self.udid)?;
            crate::paths::ensure_private(&crate::paths::run_dir().join(&self.udid))?;
            let marker = crate::paths::run_dir()
                .join(&self.udid)
                .join("wda.installed");
            let installed = std::fs::read_to_string(&marker).is_ok_and(|s| s.trim() == profile.sha)
                && goios::apps(&self.udid, false)?.contains(&bundle);
            if !installed {
                if !crate::settings::get(crate::settings::AUTO_WDA) {
                    return Err(Error::actionable(
                        "WebDriverAgent isn't installed on this iPhone and auto-install is off (Shipyard → Settings → Plugins → Mobile)",
                    ));
                }
                let app = signing::signed_wda(&profile)?;
                log(&format!("installing WebDriverAgent on {}", self.udid));
                goios::install(&self.udid, &app)?;
                std::fs::write(&marker, &profile.sha)?;
            }
            goios::ensure_image(&self.udid)?;
            let log_file = crate::paths::run_dir().join(&self.udid).join("wda.log");
            let pid = goios::spawn_wda(&self.udid, &bundle, &log_file)?;
            std::fs::write(&pid_path, pid.to_string())?;
            log(&format!(
                "starting WebDriverAgent on {} (pid {pid})",
                self.udid
            ));
        }
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if self.ready() {
                return Ok(());
            }
            sleep_ms(500);
        }
        Err(Error::other(format!(
            "WebDriverAgent did not start on {}. If the iPhone asks, trust the developer in Settings → General → VPN & Device Management, and keep it unlocked",
            self.udid
        )))
    }

    /// The current session (shared with other processes through /status).
    fn session(&mut self) -> Result<String> {
        self.ensure()?;
        if let Some(s) = &self.session {
            return Ok(s.clone());
        }
        let status = self.request("GET", "/status", None)?;
        let id = match status.get("sessionId").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => self
                .request(
                    "POST",
                    "/session",
                    Some(json!({"capabilities": {"alwaysMatch": {}}})),
                )?
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::other("wda returned no session"))?
                .to_string(),
        };
        self.session = Some(id.clone());
        Ok(id)
    }

    /// Runs a session call, once more with a fresh session if it went away.
    fn in_session(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        for attempt in 0..2 {
            let sid = self.session()?;
            match self.request(method, &format!("/session/{sid}{path}"), body.clone()) {
                Ok(v) => return Ok(v),
                Err(e) if attempt == 0 && e.message.to_lowercase().contains("session") => {
                    self.session = None
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    pub fn active_app(&mut self) -> Result<String> {
        self.ensure()?;
        let v = self.request("GET", "/wda/activeAppInfo", None)?;
        Ok(v.pointer("/value/bundleId")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string())
    }

    /// Whether the screen is locked (apps can't be opened then).
    pub fn locked(&mut self) -> bool {
        self.ensure().is_ok()
            && self
                .request("GET", "/wda/locked", None)
                .is_ok_and(|v| v.get("value") == Some(&Value::Bool(true)))
    }

    pub fn source(&mut self) -> Result<Value> {
        self.ensure()?;
        self.request("GET", "/source?format=json", None)
    }

    pub fn screenshot(&mut self) -> Result<Vec<u8>> {
        self.ensure()?;
        let v = self.request("GET", "/screenshot", None)?;
        Ok(b64decode(
            v.get("value").and_then(Value::as_str).unwrap_or(""),
        ))
    }

    /// Screen size in points.
    pub fn size(&mut self) -> Result<(f64, f64)> {
        if let Some(s) = self.size {
            return Ok(s);
        }
        let v = self.in_session("GET", "/window/size", None)?;
        let w = v
            .pointer("/value/width")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let h = v
            .pointer("/value/height")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        self.size = Some((w, h));
        Ok((w, h))
    }

    fn pointer(&mut self, steps: Vec<Value>) -> Result<()> {
        self.in_session(
            "POST",
            "/actions",
            Some(json!({"actions": [{"type": "pointer", "id": "finger1", "parameters": {"pointerType": "touch"}, "actions": steps}]})),
        )?;
        Ok(())
    }

    pub fn tap(&mut self, x: i64, y: i64, hold_ms: u64) -> Result<()> {
        self.pointer(vec![
            json!({"type": "pointerMove", "duration": 0, "x": x, "y": y}),
            json!({"type": "pointerDown", "button": 0}),
            json!({"type": "pause", "duration": hold_ms}),
            json!({"type": "pointerUp", "button": 0}),
        ])
    }

    pub fn double_tap(&mut self, x: i64, y: i64) -> Result<()> {
        self.tap(x, y, 50)?;
        sleep_ms(80);
        self.tap(x, y, 50)
    }

    fn drag(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) -> Result<()> {
        self.pointer(vec![
            json!({"type": "pointerMove", "duration": 0, "x": x0 as i64, "y": y0 as i64}),
            json!({"type": "pointerDown", "button": 0}),
            json!({"type": "pause", "duration": 100}),
            json!({"type": "pointerMove", "duration": 400, "x": x1 as i64, "y": y1 as i64}),
            json!({"type": "pointerUp", "button": 0}),
        ])
    }

    pub fn swipe(&mut self, d: Direction) -> Result<()> {
        let (w, h) = self.size()?;
        let (cx, cy) = (w / 2.0, h / 2.0);
        match d {
            Direction::Up => self.drag(cx, h * 0.8, cx, h * 0.2),
            Direction::Down => self.drag(cx, h * 0.2, cx, h * 0.8),
            Direction::Left => self.drag(w * 0.8, cy, w * 0.2, cy),
            Direction::Right => self.drag(w * 0.2, cy, w * 0.8, cy),
        }
    }

    pub fn swipe_from(
        &mut self,
        x: i64,
        y: i64,
        d: Direction,
        distance: Option<i64>,
    ) -> Result<()> {
        let (w, h) = self.size()?;
        let (x, y) = (x as f64, y as f64);
        let dy = distance
            .filter(|&v| v != 0)
            .map(|v| v as f64)
            .unwrap_or(h * 0.3);
        let dx = distance
            .filter(|&v| v != 0)
            .map(|v| v as f64)
            .unwrap_or(w * 0.3);
        match d {
            Direction::Up => self.drag(x, y, x, (y - dy).max(0.0)),
            Direction::Down => self.drag(x, y, x, (y + dy).min(h)),
            Direction::Left => self.drag(x, y, (x - dx).max(0.0), y),
            Direction::Right => self.drag(x, y, (x + dx).min(w), y),
        }
    }

    pub fn press_button(&mut self, button: &str) -> Result<()> {
        match button.to_ascii_uppercase().as_str() {
            "HOME" => {
                self.ensure()?;
                self.request("POST", "/wda/homescreen", None)?;
            }
            "VOLUME_UP" => {
                self.in_session(
                    "POST",
                    "/wda/pressButton",
                    Some(json!({"name": "volumeUp"})),
                )?;
            }
            "VOLUME_DOWN" => {
                self.in_session(
                    "POST",
                    "/wda/pressButton",
                    Some(json!({"name": "volumeDown"})),
                )?;
            }
            "ENTER" => {
                self.in_session("POST", "/wda/keys", Some(json!({"value": ["\n"]})))?;
            }
            other => {
                return Err(Error::actionable(format!(
                    "{other} doesn't exist on an iPhone: use HOME, ENTER, VOLUME_UP or VOLUME_DOWN, or tap the app's own back button"
                )));
            }
        }
        Ok(())
    }

    /// Replaces the focused field's text (Unicode ok); types at the cursor
    /// when no field reports focus.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let focused = self.in_session(
            "POST",
            "/element",
            Some(json!({"using": "predicate string", "value": "hasKeyboardFocus == 1"})),
        );
        let el = focused.ok().and_then(|v| {
            let e = v.get("value")?;
            e.get("ELEMENT")
                .or_else(|| e.get("element-6066-11e4-a52e-4f735466cecf"))
                .and_then(Value::as_str)
                .map(String::from)
        });
        match el {
            Some(id) => {
                self.in_session("POST", &format!("/element/{id}/clear"), None)?;
                self.in_session(
                    "POST",
                    &format!("/element/{id}/value"),
                    Some(json!({"value": [text]})),
                )?;
            }
            None => {
                self.in_session("POST", "/wda/keys", Some(json!({"value": [text]})))?;
            }
        }
        Ok(())
    }

    pub fn launch(&mut self, bundle: &str) -> Result<()> {
        self.in_session(
            "POST",
            "/wda/apps/launch",
            Some(json!({"bundleId": bundle})),
        )
        .map_err(|e| {
            Error::actionable(format!(
                "Failed launching app with bundle id \"{bundle}\": {e}"
            ))
        })?;
        Ok(())
    }

    pub fn terminate(&mut self, bundle: &str) -> Result<()> {
        self.in_session(
            "POST",
            "/wda/apps/terminate",
            Some(json!({"bundleId": bundle})),
        )?;
        Ok(())
    }

    pub fn running(&self) -> bool {
        self.forward.is_some()
    }

    /// Lets go of the phone; stops WebDriverAgent unless another process used
    /// it lately (it listens only on the phone's loopback, but shouldn't run idle).
    pub fn stop(&mut self) {
        self.session = None;
        lock::drop_lease(&self.udid);
        if let Some(mut f) = self.forward.take() {
            let _ = f.kill();
            let _ = f.wait();
        }
        self.port = 0;
        self.last_use = None;
        if !lock::others_using(&self.udid, IDLE_STOP) {
            let pid_path = pid_file(&self.udid);
            if let Some(pid) = std::fs::read_to_string(&pid_path)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
                && alive(pid)
            {
                // the whole process group: runwda and what it started
                unsafe { libc::kill(-pid, libc::SIGTERM) };
                log(&format!("stopped WebDriverAgent on {}", self.udid));
            }
            let _ = std::fs::remove_file(pid_path);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_base64() {
        assert_eq!(super::b64decode("Zm9vYmFy"), b"foobar");
        assert_eq!(super::b64decode("Zm8=\n"), b"fo");
    }
}
