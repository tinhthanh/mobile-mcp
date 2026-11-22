//! Appium's UiAutomator2 server, an instrumentation that keeps UiAutomation
//! alive on the device: page source in ~0.1–0.4 s instead of ~2.7 s for
//! `uiautomator dump`, and setText types Unicode (Vietnamese), which
//! `input text` cannot.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::adb::{Device, adb_path};
use crate::fetch::Pinned;
use crate::lock;
use crate::util::{Error, Result, log, sleep_ms};

pub const VERSION: &str = "10.6.6";
const SERVER_PACKAGE: &str = "io.appium.uiautomator2.server";
const TEST_PACKAGE: &str = "io.appium.uiautomator2.server.test";
const RUNNER: &str = "io.appium.uiautomator2.server.test/androidx.test.runner.AndroidJUnitRunner";

/// The official release APKs (GitHub release == npm package apks; the npm
/// package has SLSA provenance).
pub const SERVER_APK: Pinned = Pinned {
    url: "https://github.com/appium/appium-uiautomator2-server/releases/download/v10.6.6/appium-uiautomator2-server-v10.6.6.apk",
    sha256: "8ff760a2a86b487f53090fbdcd5b0360e67d02bb811887d527a9557b0d59c80d",
};
pub const TEST_APK: Pinned = Pinned {
    url: "https://github.com/appium/appium-uiautomator2-server/releases/download/v10.6.6/appium-uiautomator2-server-debug-androidTest.apk",
    sha256: "e6f729287d72351388fd66597c35b69988b9091a7011954bf92a55d416cb1e1c",
};

const DEVICE_PORT: u16 = 6790;
const START_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// While it runs, any app on the device can reach the server on
/// localhost:6790, so it is stopped when no process has used it for a while.
pub const IDLE_STOP: Duration = Duration::from_secs(5 * 60);
/// Some screens break page source for a while ("Cannot set
/// AccessibilityNodeInfo's field 'mSealed'"): fall back to adb meanwhile.
const COOLDOWN: Duration = Duration::from_secs(60);
const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

/// Whether the pinned version of both APKs is on the device.
pub fn installed(dev: &Device) -> bool {
    dev.version_name(SERVER_PACKAGE).as_deref() == Some(VERSION)
        && dev.has_package(TEST_PACKAGE).unwrap_or(false)
}

/// Downloads (once) and installs both APKs.
pub fn install(dev: &Device) -> Result<()> {
    let dir = crate::paths::cache_dir().join("apk");
    let server = crate::fetch::cached(&SERVER_APK, &dir)?;
    let test = crate::fetch::cached(&TEST_APK, &dir)?;
    log(&format!(
        "installing UiAutomator2 {VERSION} on {}",
        dev.serial
    ));
    for apk in [&server, &test] {
        let p = apk.to_string_lossy();
        dev.adb(&["install", "-r", "-t", &p]).map_err(|e| {
            Error::other(format!(
                "couldn't install UiAutomator2 on {}: {e}",
                dev.serial
            ))
        })?;
    }
    Ok(())
}

pub struct Uia2 {
    dev: Device,
    port: u16,
    session: Option<String>,
    instrument: Option<Child>,
    disabled_until: Option<Instant>,
    pub last_use: Option<Instant>,
    agent: ureq::Agent,
}

impl Uia2 {
    pub fn new(serial: &str) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(REQUEST_TIMEOUT))
                .http_status_as_error(false)
                .build(),
        );
        Uia2 {
            dev: Device::new(serial),
            port: 0,
            session: None,
            instrument: None,
            disabled_until: None,
            last_use: None,
            agent,
        }
    }

    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let err = |e: ureq::Error| Error::other(format!("uia2 {method} {path}: {e}"));
        let resp = match (method, body) {
            ("GET", _) => self.agent.get(&url).call().map_err(err)?,
            ("DELETE", _) => self.agent.delete(&url).call().map_err(err)?,
            (_, Some(b)) => self.agent.post(&url).send_json(b).map_err(err)?,
            (_, None) => self.agent.post(&url).send_empty().map_err(err)?,
        };
        let status = resp.status();
        let json: Value = resp
            .into_body()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_json()
            .map_err(|e| Error::other(format!("uia2 {method} {path}: {e}")))?;
        let value = json.get("value").cloned().unwrap_or(Value::Null);
        if !status.is_success() || value.get("error").is_some() {
            let msg = value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| status.to_string());
            return Err(Error::other(format!("uia2 {method} {path}: {msg}")));
        }
        Ok(value)
    }

    fn is_up(&self) -> bool {
        self.port != 0
            && self
                .request("GET", "/status", None)
                .is_ok_and(|v| v.get("ready") == Some(&Value::Bool(true)))
    }

    fn start(&mut self) -> Result<()> {
        if self.port == 0 {
            let out = self
                .dev
                .adb_text(&["forward", "tcp:0", &format!("tcp:{DEVICE_PORT}")])?;
            self.port = out
                .trim()
                .parse()
                .map_err(|_| Error::other(format!("adb forward said {out:?}")))?;
        }
        // another agent's process may have started it already
        if self.is_up() {
            return Ok(());
        }
        log(&format!(
            "starting uiautomator2 server on {}, local port {}",
            self.dev.serial, self.port
        ));
        self.dev.shell(&["am", "force-stop", SERVER_PACKAGE])?;
        let child = Command::new(adb_path()?)
            .args([
                "-s",
                &self.dev.serial,
                "shell",
                "am",
                "instrument",
                "-w",
                "-e",
                "disableAnalytics",
                "true",
                RUNNER,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.instrument = Some(child);
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if self.is_up() {
                return Ok(());
            }
            sleep_ms(300);
        }
        Err(Error::other("uiautomator2 server did not start"))
    }

    fn session(&mut self) -> Result<String> {
        if let Some(s) = &self.session {
            return Ok(s.clone());
        }
        self.start()?;
        // the server holds one session: share it with other processes rather
        // than replacing theirs
        let existing = self.request("GET", "/sessions", None).ok().and_then(|v| {
            v.as_array()
                .and_then(|a| a.first())
                .and_then(|s| s.get("id"))
                .and_then(Value::as_str)
                .map(String::from)
        });
        let id = match existing {
            Some(id) => id,
            None => {
                let created = self.request(
                    "POST",
                    "/session",
                    Some(json!({"capabilities": {"alwaysMatch": {"platformName": "Android"}}})),
                )?;
                created
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| Error::other("uia2 returned no session id"))?
            }
        };
        // the default waits for an idle ui, which is what makes `uiautomator dump`
        // slow; ignoreUnimportantViews cuts page source from ~500 to ~170 ms and only
        // drops unlabelled views, which the compact screen mostly skips anyway
        let all_views = std::env::var_os("MOBILEMCP_ALL_VIEWS").is_some();
        self.request(
            "POST",
            &format!("/session/{id}/appium/settings"),
            Some(json!({"settings": {"waitForIdleTimeout": 0, "waitForSelectorTimeout": 0, "ignoreUnimportantViews": !all_views}})),
        )?;
        self.session = Some(id.clone());
        Ok(id)
    }

    /// Runs `f` against a live session, recreating it when the server or the
    /// session went away. None means "use adb instead".
    fn with_session<T>(&mut self, f: impl Fn(&Self, &str) -> Result<T>) -> Option<T> {
        if self.disabled_until.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        self.last_use = Some(Instant::now());
        lock::touch_lease(&self.dev.serial);
        for attempt in 1..=3 {
            let result = self.session().and_then(|id| f(self, &id));
            match result {
                Ok(v) => return Some(v),
                Err(e) => {
                    // source fails now and then mid-transition; drop the session only when
                    // it or the server is gone (another process may have replaced it)
                    log(&format!("uia2 failed (attempt {attempt}): {e}"));
                    if e.message.to_lowercase().contains("session") || !self.is_up() {
                        self.session = None;
                    }
                    sleep_ms(200);
                }
            }
        }
        // uiautomator dump cannot run while the server holds UiAutomation
        self.disabled_until = Some(Instant::now() + COOLDOWN);
        self.stop();
        None
    }

    pub fn source(&mut self) -> Option<String> {
        self.with_session(|me, id| {
            me.request("GET", &format!("/session/{id}/source"), None)?
                .as_str()
                .map(String::from)
                .ok_or_else(|| Error::other("uia2 source is not text"))
        })
    }

    /// Replaces the focused input's text. Pasting through the clipboard would
    /// append, but it clobbers the user's clipboard.
    pub fn set_focused_text(&mut self, text: &str) -> bool {
        self.with_session(|me, id| {
            let el = me.request(
                "POST",
                &format!("/session/{id}/element"),
                Some(json!({"strategy": "-android uiautomator", "selector": "new UiSelector().focused(true)", "context": "", "multiple": false})),
            )?;
            let el_id = el
                .get(ELEMENT_KEY)
                .or_else(|| el.get("ELEMENT"))
                .and_then(Value::as_str)
                .ok_or_else(|| Error::other("no focused element"))?
                .to_string();
            me.request("POST", &format!("/session/{id}/element/{el_id}/value"), Some(json!({"text": text, "replace": true})))?;
            Ok(())
        })
        .is_some()
    }

    pub fn running(&self) -> bool {
        self.port != 0
    }

    /// Lets go of the server; stops it unless another process used it lately.
    pub fn stop(&mut self) {
        self.session = None;
        lock::drop_lease(&self.dev.serial);
        let shared = lock::others_using(&self.dev.serial, IDLE_STOP);
        if !shared {
            let _ = self.dev.shell(&["am", "force-stop", SERVER_PACKAGE]);
            if let Some(mut c) = self.instrument.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        if self.port != 0 {
            let _ = self
                .dev
                .adb(&["forward", "--remove", &format!("tcp:{}", self.port)]);
        }
        self.port = 0;
        self.last_use = None;
    }
}
