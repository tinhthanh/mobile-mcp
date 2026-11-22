//! The `net` tool. Default: mitmdump on this computer, reached over USB through
//! `adb reverse`, so nothing listens on the LAN and no root is needed; only apps
//! that honor the system proxy show up, and TLS is passed through (host only)
//! unless the host matches `decrypt` and the app trusts the mitmproxy CA.
//! `start {app}`: PCAPdroid on the phone captures every connection of one
//! package, hosts only.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::actionable;
use crate::adb::Device;
use crate::fetch::Pinned;
use crate::util::{Error, Result, log, sleep_ms, truncate};

const ADDON: &str = include_str!("../../native/net_addon.py");
const REDACT: &str = include_str!("../../native/net_redact.py");

/// F-Droid build of PCAPdroid 2.0.2 (its dex matches the developer's GitHub
/// release). GPL-3.0: fetched, never shipped with the plugin.
pub const PCAPDROID_APK: Pinned = Pinned {
    url: "https://f-droid.org/repo/com.emanuelef.remote_capture_95.apk",
    sha256: "e03f79082278b3754b4954c7537e7c9f7681e5995a44e70aa1589eca10190066",
};
const PCAPDROID: &str = "com.emanuelef.remote_capture";
const PCAPDROID_CTRL: &str = "com.emanuelef.remote_capture/.activities.CaptureCtrl";
const PCAP_DIR: &str = "/sdcard/Download/PCAPdroid";
/// Marks the proxy as ours, so a later run can clear one left behind.
const OWNER_SETTING: &str = "mobile_mcp_proxy_port";
const LIST_LIMIT: usize = 50;
const MAX_PATH: usize = 80;

fn noisy_header(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.starts_with("sec-ch-")
        || k.starts_with("sec-fetch-")
        || k.starts_with("accept")
        || k.starts_with("access-control-")
        || matches!(
            k.as_str(),
            "priority"
                | "upgrade-insecure-requests"
                | "x-content-type-options"
                | "date"
                | "connection"
                | "proxy-connection"
                | "keep-alive"
                | "via"
                | "alt-svc"
                | "server-timing"
                | "nel"
                | "report-to"
        )
}

fn find_mitmdump() -> Result<PathBuf> {
    let mut c: Vec<PathBuf> = std::env::var_os("MOBILEMCP_MITMDUMP")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    c.push("/opt/homebrew/bin/mitmdump".into());
    c.push("/usr/local/bin/mitmdump".into());
    if let Some(path) = std::env::var_os("PATH") {
        c.extend(std::env::split_paths(&path).map(|d| d.join("mitmdump")));
    }
    c.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        Error::actionable("mitmdump not found. Install mitmproxy (brew install --cask mitmproxy), or use net start {app} (PCAPdroid)")
    })
}

fn is_listening(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

fn size(bytes: u64) -> String {
    if bytes == 0 {
        "0".into()
    } else if bytes < 1024 {
        format!("{bytes}B")
    } else {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    }
}

fn read_keys(file: &Path) -> HashMap<String, String> {
    let mut keys = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(file) {
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=')
                && !k.trim().is_empty()
            {
                keys.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    keys
}

fn read_jsonl(file: &Path) -> Vec<Value> {
    std::fs::read_to_string(file)
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Where PCAPdroid api keys live: this plugin's state, then mobile-mcp-lite's file.
pub fn key_files() -> Vec<PathBuf> {
    let mut v = vec![crate::paths::state_dir().join("keys.env")];
    if let Some(f) = std::env::var_os("MOBILEMCP_KEYS_FILE") {
        v.insert(0, PathBuf::from(f));
    }
    v.push(crate::util::home().join(".config/mobile-mcp-lite/keys.env"));
    v
}

fn clear_proxy(dev: &Device, port: u16) {
    let r = (|| -> Result<()> {
        dev.shell(&["settings", "put", "global", "http_proxy", ":0"])?;
        dev.shell(&["settings", "delete", "global", "http_proxy"])?;
        dev.shell(&["settings", "delete", "global", OWNER_SETTING])?;
        if port != 0 {
            dev.adb(&["reverse", "--remove", &format!("tcp:{port}")])?;
        }
        Ok(())
    })();
    if let Err(e) = r {
        log(&format!("failed to clear proxy: {e}"));
    }
}

/// A run killed hard leaves the phone on a proxy nobody listens to, i.e.
/// without internet. Clears it when it is ours and its port is dead.
pub fn clear_stale_proxy(dev: &Device) {
    let proxy = dev
        .shell(&["settings", "get", "global", "http_proxy"])
        .unwrap_or_default();
    let owned = dev
        .shell(&["settings", "get", "global", OWNER_SETTING])
        .unwrap_or_default();
    let Ok(port) = owned.trim().parse::<u16>() else {
        return;
    };
    if port == 0 || proxy.trim() != format!("127.0.0.1:{port}") || is_listening(port) {
        return;
    }
    log(&format!(
        "clearing stale proxy {} left by an earlier run",
        proxy.trim()
    ));
    clear_proxy(dev, port);
}

pub struct NetCapture {
    dev: Device,
    proc: Arc<Mutex<Option<Child>>>,
    port: u16,
    dir: Option<PathBuf>,
    cursor: i64,
    /// Set while PCAPdroid captures one app.
    pcap_name: String,
}

impl NetCapture {
    pub fn new(serial: &str) -> Self {
        NetCapture {
            dev: Device::new(serial),
            proc: Arc::new(Mutex::new(None)),
            port: 0,
            dir: None,
            cursor: 0,
            pcap_name: String::new(),
        }
    }

    fn mitm_running(&self) -> bool {
        self.proc
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn running(&self) -> bool {
        self.mitm_running() || !self.pcap_name.is_empty()
    }

    fn log_file(&self) -> PathBuf {
        self.dir.clone().unwrap_or_default().join("log.jsonl")
    }

    fn detail_file(&self) -> PathBuf {
        self.dir.clone().unwrap_or_default().join("detail.jsonl")
    }

    fn new_dir(&mut self) -> Result<PathBuf> {
        self.remove_files();
        let d = std::env::temp_dir().join(format!(
            "berth-mobile-net-{}-{}",
            std::process::id(),
            crate::util::unix_secs()
        ));
        crate::paths::ensure_private(&d)?;
        self.dir = Some(d.clone());
        Ok(d)
    }

    /// Each phone has its own key: PCAPDROID_API_KEY_<serial> wins over
    /// PCAPDROID_API_KEY, from the environment first, then the key files.
    fn api_key(&self) -> Option<String> {
        let safe: String = self
            .dev
            .serial
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let names = [
            format!("PCAPDROID_API_KEY_{safe}"),
            "PCAPDROID_API_KEY".to_string(),
        ];
        if let Some(v) = names
            .iter()
            .find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
        {
            return Some(v);
        }
        key_files().iter().find_map(|f| {
            let keys = read_keys(f);
            names.iter().find_map(|n| keys.get(n).cloned())
        })
    }

    fn pcapdroid(&self, action: &str, extras: &[&str]) -> Result<()> {
        let key = self.api_key();
        let mut args = vec!["am", "start", "-e", "action", action];
        if let Some(k) = key.as_deref() {
            args.extend(["-e", "api_key", k]);
        }
        args.extend_from_slice(extras);
        args.extend(["-n", PCAPDROID_CTRL]);
        self.dev.shell(&args)?;
        Ok(())
    }

    fn start_app(&mut self, app: &str) -> Result<String> {
        if !self.dev.has_package(PCAPDROID)? {
            let apk = crate::fetch::cached(&PCAPDROID_APK, &crate::paths::cache_dir().join("apk"))?;
            log(&format!("installing PCAPdroid on {}", self.dev.serial));
            self.dev.adb(&["install", &apk.to_string_lossy()])?;
            let _ = self.dev.shell(&[
                "pm",
                "grant",
                PCAPDROID,
                "android.permission.POST_NOTIFICATIONS",
            ]);
        }
        self.new_dir()?;
        self.pcap_name = format!("mobile-mcp-{}.pcap", crate::util::unix_secs());
        let name = self.pcap_name.clone();
        self.pcapdroid(
            "start",
            &[
                "-e",
                "app_filter",
                app,
                "-e",
                "pcap_dump_mode",
                "pcap_file",
                "-e",
                "pcap_name",
                &name,
            ],
        )?;
        sleep_ms(1500);
        let consent = if self.api_key().is_some() {
            ""
        } else {
            " (no PCAPdroid api key: approve the prompts on the phone, and the VPN request the first time)"
        };
        Ok(format!(
            "capturing {app} with PCAPdroid, hosts only{consent}"
        ))
    }

    /// Pulls the pcap (also while it is being written) and summarizes it per host.
    fn app_hosts(&self, filter: Option<&str>) -> String {
        let local = self
            .dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join("capture.pcap");
        let remote = format!(
            "{PCAP_DIR}/{}",
            if self.pcap_name.is_empty() {
                "missing"
            } else {
                &self.pcap_name
            }
        );
        if self
            .dev
            .adb(&["pull", &remote, &local.to_string_lossy()])
            .is_err()
        {
            return "no traffic captured yet".into();
        }
        let data = std::fs::read(&local).unwrap_or_default();
        let wanted = filter.map(str::to_lowercase);
        let lines: Vec<String> = crate::pcap::summarize(&data)
            .into_iter()
            .filter(|l| wanted.as_ref().is_none_or(|w| l.to_lowercase().contains(w)))
            .take(LIST_LIMIT)
            .collect();
        if lines.is_empty() {
            "no traffic captured yet".into()
        } else {
            lines.join("\n")
        }
    }

    pub fn start(&mut self, decrypt: Option<&str>, app: Option<&str>) -> Result<String> {
        if self.running() {
            return Ok("already capturing; stop first".into());
        }
        if let Some(app) = app {
            return self.start_app(app);
        }
        if let Some(d) = decrypt
            && let Err(e) = regex_lite::Regex::new(d)
        {
            actionable!("decrypt is not a valid regex: {e}");
        }
        let mitmdump = find_mitmdump()?;
        let dir = self.new_dir()?;
        std::fs::write(dir.join("net_addon.py"), ADDON)?;
        std::fs::write(dir.join("net_redact.py"), REDACT)?;
        self.port = crate::util::free_port()?;
        self.cursor = 0;
        let child = Command::new(mitmdump)
            .args([
                "-q",
                "--listen-host",
                "127.0.0.1",
                "-p",
                &self.port.to_string(),
                "-s",
            ])
            .arg(dir.join("net_addon.py"))
            .env("NET_LOG", self.log_file())
            .env("NET_DETAIL", self.detail_file())
            .env("NET_DECRYPT", decrypt.unwrap_or(""))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        *self.proc.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        for _ in 0..50 {
            if is_listening(self.port) {
                break;
            }
            sleep_ms(100);
        }
        if !is_listening(self.port) {
            self.stop_process();
            return Err(Error::other("mitmdump did not start"));
        }
        self.dev.adb(&[
            "reverse",
            &format!("tcp:{}", self.port),
            &format!("tcp:{}", self.port),
        ])?;
        self.dev.shell(&[
            "settings",
            "put",
            "global",
            OWNER_SETTING,
            &self.port.to_string(),
        ])?;
        self.dev.shell(&[
            "settings",
            "put",
            "global",
            "http_proxy",
            &format!("127.0.0.1:{}", self.port),
        ])?;
        self.watch();
        Ok(format!("capturing via 127.0.0.1:{}", self.port)
            + &decrypt
                .map(|d| format!(", decrypting hosts matching /{d}/"))
                .unwrap_or_else(|| ", https hosts only (no decrypt)".into()))
    }

    /// Clears the proxy as soon as mitmdump exits: a proxy nobody listens to
    /// cuts the phone off the internet.
    fn watch(&self) {
        let proc = self.proc.clone();
        let serial = self.dev.serial.clone();
        let port = self.port;
        std::thread::spawn(move || {
            loop {
                sleep_ms(500);
                let mut g = proc.lock().unwrap_or_else(|e| e.into_inner());
                let Some(child) = g.as_mut() else { return };
                if let Ok(Some(status)) = child.try_wait() {
                    log(&format!("mitmdump exited with {status}"));
                    *g = None;
                    drop(g);
                    clear_proxy(&Device::new(&serial), port);
                    return;
                }
            }
        });
    }

    /// New requests since the previous list; TLS passthrough grouped per host.
    pub fn list(&mut self, filter: Option<&str>, all: bool) -> String {
        if !self.pcap_name.is_empty() {
            return self.app_hosts(filter);
        }
        let records = read_jsonl(&self.log_file());
        let id = |r: &Value| r.get("id").and_then(Value::as_i64).unwrap_or(0);
        let fresh: Vec<&Value> = records
            .iter()
            .filter(|r| all || id(r) > self.cursor)
            .collect();
        if let Some(last) = records.last() {
            self.cursor = id(last);
        }
        let wanted = filter.map(str::to_lowercase);
        let s = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |r: &Value, k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
        let mut lines = Vec::new();
        let mut tls: Vec<(String, usize)> = Vec::new();
        for r in fresh {
            let host = s(r, "h");
            let path = s(r, "p");
            if wanted
                .as_ref()
                .is_some_and(|w| !format!("{host}{path}").to_lowercase().contains(w))
            {
                continue;
            }
            if r.get("tls").and_then(Value::as_bool) == Some(true) {
                match tls.iter_mut().find(|(h, _)| *h == host) {
                    Some(e) => e.1 += 1,
                    None => tls.push((host, 1)),
                }
                continue;
            }
            let p = truncate(&path, MAX_PATH);
            let line = if r.get("err").is_some() {
                format!("#{} {} ERR {host}{p} {}", id(r), s(r, "m"), s(r, "err"))
            } else {
                let ct = s(r, "ct");
                format!(
                    "#{} {} {} {host}{p} {} {}ms{}",
                    id(r),
                    s(r, "m"),
                    n(r, "s"),
                    size(n(r, "r")),
                    n(r, "ms"),
                    if ct.is_empty() {
                        String::new()
                    } else {
                        format!(" {ct}")
                    }
                )
            };
            lines.push(line);
        }
        let omitted = lines.len().saturating_sub(LIST_LIMIT);
        let mut out: Vec<String> = Vec::new();
        if omitted > 0 {
            out.push(format!("({omitted} older omitted)"));
        }
        out.extend(lines.into_iter().skip(omitted));
        if !tls.is_empty() {
            let hosts: Vec<String> = tls
                .into_iter()
                .map(|(h, n)| if n > 1 { format!("{h}×{n}") } else { h })
                .collect();
            out.push(format!("tls (not decrypted): {}", hosts.join(", ")));
        }
        let state = if self.mitm_running() {
            ""
        } else {
            " (capture stopped)"
        };
        if out.is_empty() {
            format!("no new requests{state}")
        } else {
            out.join("\n") + state
        }
    }

    pub fn show(&self, id: i64, all: bool) -> Result<String> {
        let details = read_jsonl(&self.detail_file());
        let Some(d) = details
            .iter()
            .find(|d| d.get("id").and_then(Value::as_i64) == Some(id))
        else {
            actionable!("No details for #{id}; only decrypted or plain http requests have them");
        };
        let headers = |v: Option<&Value>| -> String {
            v.and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter(|(k, _)| all || !noisy_header(k))
                        .map(|(k, v)| format!("  {k}: {}", v.as_str().unwrap_or("")))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };
        let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut out = vec![
            s("url"),
            "request headers:".into(),
            headers(d.get("req_headers")),
        ];
        if !s("req_body").is_empty() {
            out.extend(["request body:".into(), s("req_body")]);
        }
        if !s("error").is_empty() {
            out.push(format!("error: {}", s("error")));
        } else {
            out.extend(["response headers:".into(), headers(d.get("resp_headers"))]);
        }
        if !s("resp_body").is_empty() {
            out.extend(["response body:".into(), s("resp_body")]);
        }
        Ok(out.join("\n"))
    }

    pub fn stop(&mut self) -> Result<String> {
        if !self.pcap_name.is_empty() {
            self.pcapdroid("stop", &[])?;
            let hosts = self.app_hosts(None);
            let _ = self
                .dev
                .shell(&["rm", "-f", &format!("{PCAP_DIR}/{}", self.pcap_name)]);
            self.pcap_name.clear();
            return Ok(format!(
                "stopped PCAPdroid, capture file removed from the device\n{hosts}"
            ));
        }
        let total = read_jsonl(&self.log_file()).len();
        self.stop_process();
        Ok(format!(
            "stopped, proxy cleared. {total} records kept until the next start"
        ))
    }

    fn stop_process(&mut self) {
        let child = self.proc.lock().unwrap_or_else(|e| e.into_inner()).take();
        clear_proxy(&self.dev, self.port);
        if let Some(mut c) = child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// On exit: stop capturing and delete what was captured.
    pub fn shutdown(&mut self) {
        if !self.pcap_name.is_empty() {
            let _ = self.stop();
        }
        if self.mitm_running() {
            self.stop_process();
        }
        self.remove_files();
    }

    pub fn remove_files(&mut self) {
        if let Some(d) = self.dir.take() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("berth-mobile-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn with_logs(name: &str, summaries: &[Value], details: &[Value]) -> NetCapture {
        let dir = temp_dir(name);
        let lines = |v: &[Value]| v.iter().map(|x| x.to_string() + "\n").collect::<String>();
        std::fs::write(dir.join("log.jsonl"), lines(summaries)).unwrap();
        std::fs::write(dir.join("detail.jsonl"), lines(details)).unwrap();
        let mut c = NetCapture::new("SERIAL1");
        c.dir = Some(dir);
        c
    }

    #[test]
    fn redacts_credentials_in_bodies() {
        // the python half runs inside mitmdump; check it with any python3 there is
        let dir = temp_dir("redact");
        std::fs::write(dir.join("net_redact.py"), REDACT).unwrap();
        let input = [
            r#"{"access_token": "abc.def", "user": "an", "password": "p\"q"}"#,
            r#"{"data": "{\"password\":\"hunter2\"}"}"#,
            "user=an&refresh_token=zzz&page=2",
            "Authorization: Bearer eyJhbGciOi.xyz",
        ]
        .join("\n");
        let out = std::process::Command::new("python3")
            .args(["-I", "-c", "import sys; sys.path.insert(0, sys.argv[1]); from net_redact import redact; sys.stdout.write(redact(sys.argv[2]))"])
            .arg(&dir)
            .arg(&input)
            .output();
        let Ok(out) = out else { return };
        let out = String::from_utf8_lossy(&out.stdout);
        for secret in ["abc.def", "p\\\"q", "hunter2", "zzz", "eyJhbGciOi"] {
            assert!(!out.contains(secret), "{secret} leaked: {out}");
        }
        assert!(out.contains(r#""user": "an""#));
        assert!(out.contains("page=2"));
    }

    #[test]
    fn lists_new_requests_groups_tls_and_filters() {
        let mut c = with_logs(
            "list",
            &[
                serde_json::json!({"id": 1, "t": 0, "m": "GET", "s": 200, "h": "api.example.com", "p": "/v1/users", "r": 1300, "ms": 140, "ct": "application/json"}),
                serde_json::json!({"id": 2, "t": 0, "tls": true, "h": "cdn.example.com"}),
                serde_json::json!({"id": 3, "t": 0, "tls": true, "h": "cdn.example.com"}),
                serde_json::json!({"id": 4, "t": 0, "m": "POST", "s": 0, "h": "api.example.com", "p": "/v1/login", "err": "connection reset"}),
            ],
            &[],
        );
        assert_eq!(
            c.list(None, false),
            "#1 GET 200 api.example.com/v1/users 1.3KB 140ms application/json\n#4 POST ERR api.example.com/v1/login connection reset\ntls (not decrypted): cdn.example.com×2 (capture stopped)"
        );
        assert_eq!(c.list(None, false), "no new requests (capture stopped)");
        let log = c.log_file();
        let mut text = std::fs::read_to_string(&log).unwrap();
        text.push_str(&(serde_json::json!({"id": 5, "t": 0, "m": "GET", "s": 304, "h": "img.example.com", "p": "/a.png", "r": 0, "ms": 9}).to_string() + "\n"));
        std::fs::write(&log, text).unwrap();
        assert_eq!(
            c.list(None, false),
            "#5 GET 304 img.example.com/a.png 0 9ms (capture stopped)"
        );
        assert_eq!(
            c.list(Some("users"), true),
            "#1 GET 200 api.example.com/v1/users 1.3KB 140ms application/json (capture stopped)"
        );
        c.remove_files();
    }

    #[test]
    fn show_drops_noise_headers_unless_all() {
        let mut c = with_logs(
            "show",
            &[],
            &[serde_json::json!({
                "id": 1,
                "url": "https://api.example.com/v1/users",
                "req_headers": {"sec-fetch-mode": "cors", "accept": "*/*", "authorization": "[redacted]"},
                "req_body": "",
                "resp_headers": {"content-type": "application/json", "date": "today"},
                "resp_body": "{}",
            })],
        );
        let short = c.show(1, false).unwrap();
        assert!(short.contains("authorization: [redacted]"));
        assert!(short.contains("content-type: application/json"));
        assert!(!short.contains("sec-fetch-mode"));
        assert!(!short.contains("date:"));
        assert!(c.show(1, true).unwrap().contains("sec-fetch-mode: cors"));
        c.remove_files();
    }

    #[test]
    fn api_key_per_device_from_env_then_keys_file() {
        let dir = temp_dir("keys");
        let file = dir.join("keys.env");
        std::fs::write(
            &file,
            "# comment\nPCAPDROID_API_KEY_OTHER=nope\nPCAPDROID_API_KEY_ABC_5555=from-file\n",
        )
        .unwrap();
        unsafe { std::env::set_var("MOBILEMCP_KEYS_FILE", &file) };
        assert_eq!(
            NetCapture::new("ABC:5555").api_key().as_deref(),
            Some("from-file")
        );
        unsafe { std::env::set_var("PCAPDROID_API_KEY_ABC_5555", "from-env") };
        assert_eq!(
            NetCapture::new("ABC:5555").api_key().as_deref(),
            Some("from-env")
        );
        unsafe { std::env::remove_var("PCAPDROID_API_KEY_ABC_5555") };
        unsafe { std::env::remove_var("MOBILEMCP_KEYS_FILE") };
        let _ = std::fs::remove_dir_all(dir);
    }
}
