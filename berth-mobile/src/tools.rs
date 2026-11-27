//! The MCP tools: definitions and what they do.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::actionable;
use crate::adb::{self, BUTTONS, Device, Direction};
use crate::ios::IosPhone;
use crate::net::NetCapture;
use crate::screen::{SCREEN_LEGEND, compact_screen, find_line};
use crate::settings;
use crate::uia2::{self, Uia2};
use crate::util::{Error, Result, log, sleep_ms};

/// Below this many lines the accessibility tree is probably useless (game, canvas, flutter).
const OCR_FALLBACK_MIN_LINES: usize = 3;
const SETTLE_MS: u64 = 500;
/// With UiAutomator2 a read is cheap enough to poll until two reads agree.
const STABLE_MIN_MS: u64 = 300;
const STABLE_MAX: Duration = Duration::from_millis(4000);
const POLL_MS: u64 = 300;
const DEFAULT_WAIT_MS: u64 = 10000;
const MAX_WAIT_MS: u64 = 30000;
const MAX_STEPS: usize = 30;
const DEVICE_CACHE: Duration = Duration::from_secs(10);
/// How long an iPhone screen read stays good for `tap {text}`.
const FRESH_FOR: Duration = Duration::from_secs(5);

pub fn instructions() -> String {
    format!(
        "Drive an Android phone (adb) or an iPhone (WebDriverAgent) plugged in over USB. device may be omitted when only one is connected.\n\
Read the screen with screen. {SCREEN_LEGEND}\n\
tap, type, swipe, key, app and steps return the new screen (or \"unchanged\"); observe=false skips it. Do not call screen right after them.\n\
Batch known sequences with steps; use wait instead of polling screen.\n\
On an iPhone, app takes bundle ids (com.apple.Preferences), there is no BACK key (tap the app's back button), and net is not available."
    )
}

pub enum Output {
    Text(String),
    Image { data: String, mime: &'static str },
}

#[derive(Default)]
pub struct State {
    known: HashSet<String>,
    uia2: HashMap<String, Option<Uia2>>,
    last_screen: HashMap<String, String>,
    captures: HashMap<String, NetCapture>,
    devices: Option<(Instant, Vec<String>)>,
    ios_ids: HashSet<String>,
    ios: HashMap<String, IosPhone>,
    /// An iPhone's last read screen, until the next action: reading one
    /// takes ~2 s, so `tap {text}` right after `screen` reuses it.
    fresh: HashMap<String, (Instant, String)>,
}

/// iPhones attached, when this computer is set up to sign for them.
fn ios_devices() -> Vec<String> {
    if !crate::signing::available() {
        return Vec::new();
    }
    crate::goios::list().unwrap_or_else(|e| {
        log(&format!("go-ios list failed: {e}"));
        Vec::new()
    })
}

fn is_mirror(id: &str) -> bool {
    id == crate::mirroring::DEVICE_ID
}

/// PNG width in pixels, from its header.
fn png_width(png: &[u8]) -> Option<u32> {
    png.get(16..20)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

pub type Shared = Arc<Mutex<State>>;

/// Stops UiAutomator2 servers this process left idle.
pub fn spawn_idle_reaper(state: Shared) {
    std::thread::spawn(move || {
        loop {
            sleep_ms(30_000);
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            for u in s.uia2.values_mut().flatten() {
                if u.running() && u.last_use.is_some_and(|t| t.elapsed() >= uia2::IDLE_STOP) {
                    u.stop();
                }
            }
            for p in s.ios.values_mut() {
                if p.running()
                    && p.last_use
                        .is_some_and(|t| t.elapsed() >= crate::ios::IDLE_STOP)
                {
                    p.stop();
                }
            }
        }
    });
}

/// Lets go of the phones on exit: UiAutomator2 (unless shared) and captures.
pub fn shutdown(state: &Shared) {
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    for u in s.uia2.values_mut().flatten() {
        u.stop();
    }
    for c in s.captures.values_mut() {
        c.shutdown();
    }
    for p in s.ios.values_mut() {
        p.stop();
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    let mut v = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        v["required"] = json!(required);
    }
    v
}

/// tools/list
pub fn definitions() -> Value {
    let device = json!({"type": "string"});
    let observe = json!({"type": "boolean"});
    let int = json!({"type": "integer"});
    let ro = json!({"readOnlyHint": true});
    let rw = json!({"destructiveHint": true});
    let tool = |name: &str, description: &str, schema: Value, annotations: &Value| json!({"name": name, "description": description, "inputSchema": schema, "annotations": annotations});
    json!([
        tool(
            "devices",
            "List connected devices: id model android-version.",
            obj(json!({}), &[]),
            &ro
        ),
        tool(
            "screen",
            "Read the screen as compact text. ocr=true reads text by OCR (games, canvas); image=true returns a JPEG instead, costing far more tokens.",
            obj(
                json!({"device": device, "ocr": {"type": "boolean"}, "image": {"type": "boolean"}}),
                &[]
            ),
            &ro
        ),
        tool(
            "tap",
            "Tap x,y from a screen line, or the element whose label matches text.",
            obj(
                json!({"device": device, "x": int, "y": int, "text": {"type": "string"}, "mode": {"type": "string", "enum": ["tap", "double", "long"]}, "observe": observe}),
                &[]
            ),
            &rw
        ),
        tool(
            "type",
            "Set the focused input's text (replaces it, unicode ok). submit presses ENTER.",
            obj(
                json!({"device": device, "text": {"type": "string"}, "submit": {"type": "boolean"}, "observe": observe}),
                &["text"]
            ),
            &rw
        ),
        tool(
            "swipe",
            "Swipe; up scrolls content down. Optional start x,y and distance in px.",
            obj(
                json!({"device": device, "direction": {"type": "string", "enum": ["up", "down", "left", "right"]}, "x": int, "y": int, "distance": int, "observe": observe}),
                &["direction"]
            ),
            &rw
        ),
        tool(
            "key",
            "Press a button.",
            obj(
                json!({"device": device, "button": {"type": "string", "enum": BUTTONS}, "observe": observe}),
                &["button"]
            ),
            &rw
        ),
        tool(
            "app",
            "Launch or stop a package, or list launchable packages.",
            obj(
                json!({"device": device, "action": {"type": "string", "enum": ["launch", "stop", "list"]}, "package": {"type": "string"}, "observe": observe}),
                &["action"]
            ),
            &rw
        ),
        tool(
            "wait",
            "Wait until text appears (or is gone) on screen, then return the screen.",
            obj(
                json!({"device": device, "text": {"type": "string"}, "gone": {"type": "boolean"}, "timeout": {"type": "integer", "description": "ms, default 10000"}}),
                &["text"]
            ),
            &ro
        ),
        tool(
            "steps",
            "Run actions in order, then return the final screen; stops at the first failure. Each item: \"tap <text|x,y>\", \"long <text|x,y>\", \"type <text>\", \"enter\", \"key <BUTTON>\", \"swipe <up|down|left|right>\", \"wait <text>\", \"gone <text>\", \"launch <package>\", \"stop <package>\", \"sleep <ms>\".",
            obj(
                json!({"device": device, "steps": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": MAX_STEPS}}),
                &["steps"]
            ),
            &rw
        ),
        tool(
            "net",
            "Capture HTTP(S) requests. start, use the app, list (new since last list), show {id} for headers/bodies, stop when done. Default: mitmproxy over USB for apps using the system proxy; HTTPS hosts only unless decrypt matches and the app trusts the user CA. start {app} uses PCAPdroid instead: every connection of that package, hosts only.",
            obj(
                json!({
                    "device": device,
                    "action": {"type": "string", "enum": ["start", "list", "show", "stop"]},
                    "filter": {"type": "string"},
                    "all": {"type": "boolean", "description": "list: include already listed; show: all headers"},
                    "id": int,
                    "decrypt": {"type": "string", "description": "host regex"},
                    "app": {"type": "string", "description": "package"}
                }),
                &["action"]
            ),
            &rw
        ),
    ])
}

/// Typed access to a tool call's arguments.
struct Args<'a>(&'a Value);

impl Args<'_> {
    fn str(&self, k: &str) -> Option<String> {
        self.0.get(k).and_then(Value::as_str).map(String::from)
    }
    fn req_str(&self, k: &str) -> Result<String> {
        self.str(k)
            .ok_or_else(|| Error::actionable(format!("{k} is required")))
    }
    fn int(&self, k: &str) -> Option<i64> {
        self.0
            .get(k)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
    }
    fn flag(&self, k: &str) -> bool {
        self.0.get(k).and_then(Value::as_bool).unwrap_or(false)
    }
    fn observe(&self) -> bool {
        self.0.get("observe").and_then(Value::as_bool) != Some(false)
    }
}

impl State {
    /// The device param is optional: with one device there is nothing to choose.
    fn resolve(&mut self, device: Option<String>) -> Result<String> {
        if let Some(d) = device.filter(|d| !d.is_empty()) {
            return Ok(d);
        }
        let fresh = self
            .devices
            .as_ref()
            .is_some_and(|(t, _)| t.elapsed() < DEVICE_CACHE);
        if !fresh {
            let mut ids = adb::ready_devices().unwrap_or_default();
            let ios = ios_devices();
            self.ios_ids.extend(ios.iter().cloned());
            ids.extend(ios);
            if crate::mirroring::enabled() && crate::mirroring::problem().is_none() {
                ids.push(crate::mirroring::DEVICE_ID.to_string());
            }
            self.devices = Some((Instant::now(), ids));
        }
        let ids = &self.devices.as_ref().expect("set above").1;
        if ids.len() != 1 {
            let found = if ids.is_empty() {
                "none".to_string()
            } else {
                ids.join(", ")
            };
            actionable!("Pass device: {} devices connected ({found})", ids.len());
        }
        Ok(ids[0].clone())
    }

    fn device(&mut self, id: &str) -> Result<Device> {
        if !self.known.contains(id) {
            if !adb::ready_devices()?.iter().any(|d| d == id) {
                actionable!(
                    "Device \"{id}\" not found. Use the devices tool to see available devices"
                );
            }
            self.known.insert(id.to_string());
            crate::net::clear_stale_proxy(&Device::new(id));
        }
        Ok(Device::new(id))
    }

    /// The iPhone with this UDID, or None for an Android phone.
    fn ios(&mut self, id: &str) -> Result<Option<&mut IosPhone>> {
        if !self.ios_ids.contains(id) && !self.known.contains(id) {
            let ios = ios_devices();
            self.ios_ids.extend(ios);
        }
        if !self.ios_ids.contains(id) {
            return Ok(None);
        }
        Ok(Some(
            self.ios
                .entry(id.to_string())
                .or_insert_with(|| IosPhone::new(id)),
        ))
    }

    fn is_ios(&mut self, id: &str) -> Result<bool> {
        Ok(self.ios(id)?.is_some())
    }

    fn screenshot(&mut self, id: &str) -> Result<Vec<u8>> {
        if is_mirror(id) {
            return crate::mirroring::screenshot().map(|(png, _)| png);
        }
        match self.ios(id)? {
            Some(p) => p.screenshot(),
            None => self.device(id)?.screenshot(),
        }
    }

    fn do_tap(&mut self, id: &str, x: i64, y: i64, mode: &str) -> Result<()> {
        self.fresh.remove(id);
        if is_mirror(id) {
            return match mode {
                "double" => crate::mirroring::tap(x, y, 50, 2),
                "long" => crate::mirroring::tap(x, y, 800, 1),
                _ => crate::mirroring::tap(x, y, 60, 1),
            };
        }
        match self.ios(id)? {
            Some(p) => match mode {
                "double" => p.double_tap(x, y),
                "long" => p.tap(x, y, 800),
                _ => p.tap(x, y, 50),
            },
            None => {
                let dev = self.device(id)?;
                match mode {
                    "double" => dev.double_tap(x, y),
                    "long" => dev.long_press(x, y, 800),
                    _ => dev.tap(x, y),
                }
            }
        }
    }

    fn do_swipe(
        &mut self,
        id: &str,
        d: Direction,
        from: Option<(i64, i64)>,
        distance: Option<i64>,
    ) -> Result<()> {
        self.fresh.remove(id);
        if is_mirror(id) {
            return crate::mirroring::swipe(d, from, distance);
        }
        match (self.ios(id)?, from) {
            (Some(p), Some((x, y))) => p.swipe_from(x, y, d, distance),
            (Some(p), None) => p.swipe(d),
            (None, Some((x, y))) => self.device(id)?.swipe_from(x, y, d, distance),
            (None, None) => self.device(id)?.swipe(d),
        }
    }

    fn do_key(&mut self, id: &str, button: &str) -> Result<()> {
        self.fresh.remove(id);
        if is_mirror(id) {
            return crate::mirroring::press_button(button);
        }
        match self.ios(id)? {
            Some(p) => p.press_button(button),
            None => self.device(id)?.press_button(button),
        }
    }

    fn list_apps(&mut self, id: &str) -> Result<Vec<String>> {
        if is_mirror(id) {
            actionable!(
                "iPhone Mirroring can't list apps: launch one by its Home Screen name (app launch \"Cài đặt\"), then tap its Spotlight result"
            );
        }
        if self.is_ios(id)? {
            return crate::goios::apps(id, true);
        }
        self.device(id)?.list_apps()
    }

    /// The iPhone's compact screen: WebDriverAgent's tree, OCR when it is thin.
    fn ios_screen(&mut self, id: &str, force_ocr: bool) -> Result<String> {
        let p = self.ios(id)?.expect("checked by the caller");
        let source = p.source()?;
        if let Some(path) = std::env::var_os("BERTH_MOBILE_DUMP") {
            // debugging aid: the raw tree, for fixtures
            let _ = std::fs::write(path, source.to_string());
        }
        let app = p.active_app()?;
        let mut body = crate::screen_ios::compact(&source);
        let mut via = "";
        if force_ocr || body.len() < OCR_FALLBACK_MIN_LINES {
            let png = p.screenshot()?;
            let (w, _) = p.size()?;
            // OCR reads pixels, taps take points
            let scale = png_width(&png)
                .map(|px| px as f64 / w.max(1.0))
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0);
            if let Some(ocr) = crate::ocr::ocr_lines(&png) {
                let ocr: Vec<String> = ocr
                    .into_iter()
                    .filter_map(|l| {
                        let (text, xy) = l.rsplit_once('|')?;
                        let (x, y) = xy.split_once(',')?;
                        let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?);
                        Some(format!(
                            "{text}|{},{}",
                            (x / scale) as i64,
                            (y / scale) as i64
                        ))
                    })
                    .collect();
                if force_ocr {
                    body = ocr;
                } else {
                    body.extend(ocr);
                }
                via = " (ocr)";
            }
        }
        let locked = if p.locked() {
            " (locked: ask the person to unlock the iPhone)"
        } else {
            ""
        };
        let mut out = vec![format!("app={app}{via}{locked}")];
        out.extend(body);
        Ok(out.join("\n"))
    }

    fn uia2(&mut self, id: &str) -> Result<Option<&mut Uia2>> {
        if !self.uia2.contains_key(id) {
            let dev = self.device(id)?;
            let usable = std::env::var_os("MOBILEMCP_DISABLE_UIA2").is_none() && {
                uia2::installed(&dev)
                    || (settings::get(settings::AUTO_UIA2)
                        && uia2::install(&dev)
                            .map_err(|e| log(&format!("UiAutomator2 install failed: {e}")))
                            .is_ok())
            };
            self.uia2
                .insert(id.to_string(), usable.then(|| Uia2::new(id)));
        }
        Ok(self.uia2.get_mut(id).and_then(Option::as_mut))
    }

    fn capture_screen(&mut self, id: &str, force_ocr: bool) -> Result<String> {
        if is_mirror(id) {
            let _ = force_ocr; // always OCR: the mirror is a picture
            let mut out = vec!["app=iPhone Mirroring (ocr)".to_string()];
            out.extend(crate::mirroring::screen_lines()?);
            let screen = out.join("\n");
            self.fresh
                .insert(id.to_string(), (Instant::now(), screen.clone()));
            return Ok(screen);
        }
        if self.is_ios(id)? {
            let screen = self.ios_screen(id, force_ocr)?;
            self.fresh
                .insert(id.to_string(), (Instant::now(), screen.clone()));
            return Ok(screen);
        }
        let source = self.uia2(id)?.and_then(|u| u.source());
        let dev = self.device(id)?;
        let source = match source {
            Some(s) => s,
            None => dev.uiautomator_dump()?,
        };
        let screen = compact_screen(&source)?;
        let mut body = screen.lines;
        let mut via = "";
        if (force_ocr || body.len() < OCR_FALLBACK_MIN_LINES)
            && let Some(ocr) = crate::ocr::ocr_lines(&dev.screenshot()?)
        {
            if force_ocr {
                body = ocr;
            } else {
                body.extend(ocr);
            }
            via = " (ocr)";
        }
        let mut out = vec![format!("app={}{via}", screen.app)];
        out.extend(body);
        Ok(out.join("\n"))
    }

    /// "unchanged" when the screen is what the agent saw last.
    fn report(&mut self, id: &str, text: String) -> String {
        if self.last_screen.get(id) == Some(&text) {
            if is_mirror(id) {
                return "unchanged (if the action should have changed the screen, iPhone Mirroring may have stopped passing input on: ask the person to quit it (⌘Q) and open it again)".into();
            }
            return "unchanged".into();
        }
        self.last_screen.insert(id.to_string(), text.clone());
        text
    }

    /// Waits for the ui to settle so the agent doesn't get a half-drawn transition.
    fn settled_screen(&mut self, id: &str) -> Result<String> {
        // an iPhone's tree takes ~2 s to read: settle once instead of polling
        if is_mirror(id) || self.is_ios(id)? || self.uia2(id)?.is_none() {
            sleep_ms(SETTLE_MS);
            return self.capture_screen(id, false);
        }
        sleep_ms(STABLE_MIN_MS);
        let deadline = Instant::now() + STABLE_MAX;
        let mut previous = self.capture_screen(id, false)?;
        while Instant::now() < deadline {
            let current = self.capture_screen(id, false)?;
            if current == previous {
                break;
            }
            previous = current;
        }
        Ok(previous)
    }

    fn after_action(&mut self, id: &str, observe: bool, prefix: &str) -> Result<String> {
        if !observe {
            self.last_screen.remove(id);
            return Ok(format!("{prefix}ok"));
        }
        let screen = self.settled_screen(id)?;
        Ok(format!("{prefix}{}", self.report(id, screen)))
    }

    fn tap(
        &mut self,
        id: &str,
        xy: Option<(i64, i64)>,
        text: Option<&str>,
        mode: &str,
        wait_ms: u64,
    ) -> Result<String> {
        let mut prefix = String::new();
        let (x, y) = match xy {
            Some(p) => p,
            None => {
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    actionable!("Give x,y or text")
                };
                // the target may still be animating in: look for it a little while
                let deadline = Instant::now() + Duration::from_millis(wait_ms);
                let recent = self
                    .fresh
                    .get(id)
                    .filter(|(t, _)| t.elapsed() < FRESH_FOR)
                    .map(|(_, s)| s.clone());
                let mut screen = match recent {
                    Some(s) => s,
                    None => self.capture_screen(id, false)?,
                };
                let mut found = find_line(&screen, text);
                while found.is_none() && Instant::now() < deadline {
                    sleep_ms(POLL_MS);
                    screen = self.capture_screen(id, false)?;
                    found = find_line(&screen, text);
                }
                let Some(f) = found else {
                    self.last_screen.insert(id.to_string(), screen.clone());
                    actionable!("No element matches \"{text}\". Current screen:\n{screen}\n");
                };
                prefix = format!(
                    "tapped {}{}\n",
                    f.line,
                    if f.matches > 1 {
                        format!(" (first of {} matches)", f.matches)
                    } else {
                        String::new()
                    }
                );
                (f.x, f.y)
            }
        };
        self.do_tap(id, x, y, mode)?;
        Ok(prefix)
    }

    fn type_text(&mut self, id: &str, text: &str, submit: bool) -> Result<()> {
        self.fresh.remove(id);
        if is_mirror(id) {
            crate::mirroring::type_text(text)?;
            if submit {
                crate::mirroring::press_button("ENTER")?;
            }
            return Ok(());
        }
        if let Some(p) = self.ios(id)? {
            p.type_text(text)?;
            if submit {
                p.press_button("ENTER")?;
            }
            return Ok(());
        }
        // setText handles unicode and replaces the field; `input text` is ascii only and appends
        let typed = self.uia2(id)?.is_some_and(|u| u.set_focused_text(text));
        let dev = self.device(id)?;
        if !typed {
            dev.send_keys(text)?;
        }
        if submit {
            dev.press_button("ENTER")?;
        }
        Ok(())
    }

    fn app(&mut self, id: &str, action: &str, package: Option<&str>) -> Result<()> {
        self.fresh.remove(id);
        let Some(pkg) = package.filter(|p| !p.is_empty()) else {
            actionable!("package is required for {action}")
        };
        if is_mirror(id) {
            // no bundle ids through the mirror: the app's name, through Spotlight
            return if action == "launch" {
                crate::mirroring::launch(pkg)
            } else {
                Err(Error::actionable(
                    "iPhone Mirroring can't stop apps: swipe the app away in the app switcher",
                ))
            };
        }
        adb::validate_package(pkg)?;
        if let Some(p) = self.ios(id)? {
            return if action == "launch" {
                p.launch(pkg)
            } else {
                p.terminate(pkg)
            };
        }
        let dev = self.device(id)?;
        if action == "launch" {
            dev.launch_app(pkg)
        } else {
            dev.terminate_app(pkg)
        }
    }

    /// Polls until text shows up (or is gone), returning the screen it matched on.
    fn wait_for(&mut self, id: &str, text: &str, gone: bool, timeout_ms: u64) -> Result<String> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(MAX_WAIT_MS));
        loop {
            let screen = self.capture_screen(id, false)?;
            if find_line(&screen, text).is_some() != gone {
                return Ok(screen);
            }
            if Instant::now() >= deadline {
                self.last_screen.insert(id.to_string(), screen.clone());
                actionable!(
                    "Timed out waiting for \"{text}\" to {}. Current screen:\n{screen}\n",
                    if gone { "disappear" } else { "appear" }
                );
            }
            sleep_ms(POLL_MS);
        }
    }

    fn step(&mut self, id: &str, i: usize, step: &str, notes: &mut Vec<String>) -> Result<()> {
        let (verb, rest) = match step.split_once(' ') {
            Some((v, r)) => (v.to_lowercase(), r.trim().to_string()),
            None => (step.to_lowercase(), String::new()),
        };
        let point = rest
            .split_once(',')
            .and_then(|(a, b)| Some((a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?)))
            .filter(|_| {
                rest.chars()
                    .all(|c| c.is_ascii_digit() || c == ',' || c.is_whitespace())
            });
        // let the previous action finish its transition before acting on coordinates
        if i > 0 && (point.is_some() || verb == "type" || verb == "enter") {
            if self.is_ios(id)? {
                // a read costs ~2 s on an iPhone: give the transition time instead
                sleep_ms(SETTLE_MS);
            } else {
                self.settled_screen(id)?;
            }
        }
        match verb.as_str() {
            "tap" | "long" => {
                let note = match point {
                    Some(p) => self.tap(id, Some(p), None, &verb, 0)?,
                    None => self.tap(id, None, Some(&rest), &verb, DEFAULT_WAIT_MS)?,
                };
                if !note.is_empty() {
                    notes.push(note.trim().to_string());
                }
            }
            "type" => self.type_text(id, &rest, false)?,
            "enter" => self.do_key(id, "ENTER")?,
            "key" => self.do_key(id, &rest)?,
            "swipe" => self.do_swipe(id, Direction::parse(&rest)?, None, None)?,
            "wait" | "gone" => {
                self.wait_for(id, &rest, verb == "gone", DEFAULT_WAIT_MS)?;
            }
            "launch" | "stop" => self.app(id, &verb, Some(&rest))?,
            "sleep" => sleep_ms(rest.parse::<u64>().unwrap_or(0).min(MAX_WAIT_MS)),
            _ => actionable!("unknown action \"{verb}\""),
        }
        Ok(())
    }

    fn net(&mut self, id: &str, a: &Args) -> Result<String> {
        if !settings::get(settings::NET) {
            actionable!(
                "Network capture is off for this computer. The person can turn on \"net\" for the mobile plugin in Shipyard → Settings → Plugins"
            );
        }
        if is_mirror(id) || self.is_ios(id)? {
            actionable!("net works on Android phones only");
        }
        self.device(id)?;
        let capture = self
            .captures
            .entry(id.to_string())
            .or_insert_with(|| NetCapture::new(id));
        match a.req_str("action")?.as_str() {
            "start" => {
                let app = a.str("app");
                if let Some(p) = &app {
                    adb::validate_package(p)?;
                }
                capture.start(a.str("decrypt").as_deref(), app.as_deref())
            }
            "list" => Ok(capture.list(a.str("filter").as_deref(), a.flag("all"))),
            "show" => {
                let Some(n) = a.int("id") else {
                    actionable!("id is required for show")
                };
                capture.show(n, a.flag("all"))
            }
            _ => capture.stop(),
        }
    }

    fn devices_text() -> Result<String> {
        let listed = adb::list_devices().unwrap_or_default();
        let mut iphones = Vec::new();
        if crate::goios::present() || crate::signing::available() {
            for udid in crate::goios::list().unwrap_or_default() {
                let (model, version) = crate::goios::product(&udid).unwrap_or_default();
                let mut line = format!("{udid} {model} ios{version}");
                if let Some(problem) = match crate::signing::current_profile() {
                    Err(e) => Some(e.message),
                    Ok((p, _)) => crate::signing::check_device(&p, &udid)
                        .err()
                        .map(|e| e.message),
                } {
                    line.push_str(&format!(" (not usable: {problem})"));
                }
                iphones.push(line);
            }
        }
        // an iPhone on the cable that go-ios can't list (no signing set up here,
        // not trusted, locked): say what is missing instead of nothing
        let on_usb = crate::goios::usb_count();
        if on_usb > iphones.len() {
            let why = match crate::signing::current_profile() {
                Err(e) => e.message,
                Ok(_) => "unlock it and tap Trust on the iPhone, or replug the cable".to_string(),
            };
            iphones.push(format!("iPhone on USB, not usable: {why}"));
        }
        if crate::mirroring::enabled() {
            iphones.push(match crate::mirroring::problem() {
                None => format!(
                    "{} iPhone through iPhone Mirroring (OCR; app launch takes a name; swipes move the Mac's pointer)",
                    crate::mirroring::DEVICE_ID
                ),
                Some(p) => format!("{} not usable: {p}", crate::mirroring::DEVICE_ID),
            });
        }
        if listed.is_empty() && iphones.is_empty() {
            return Ok(
                "No device. Connect one over USB with USB debugging enabled and authorized.".into(),
            );
        }
        Ok(listed
            .iter()
            .map(|d| match d.state.as_str() {
                "device" => {
                    let dev = Device::new(&d.serial);
                    format!(
                        "{} {} android{}",
                        d.serial,
                        dev.name().replace(char::is_whitespace, "_"),
                        dev.getprop("ro.build.version.release")
                    )
                }
                "unauthorized" => format!(
                    "{} unauthorized: unlock the phone and allow USB debugging",
                    d.serial
                ),
                other => format!("{} {other}", d.serial),
            })
            .chain(iphones)
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn run(&mut self, name: &str, args: &Value) -> Result<Output> {
        let a = Args(args);
        if name == "devices" {
            return Ok(Output::Text(Self::devices_text()?));
        }
        let id = self.resolve(a.str("device"))?;
        let _guard = crate::lock::lock_device(&id)?;
        let text = match name {
            "screen" => {
                if a.flag("image") {
                    let (data, mime) = crate::shot::shrink(self.screenshot(&id)?);
                    return Ok(Output::Image {
                        data: crate::util::base64_encode(&data),
                        mime,
                    });
                }
                self.last_screen.remove(&id);
                let s = self.capture_screen(&id, a.flag("ocr"))?;
                self.report(&id, s)
            }
            "tap" => {
                let xy = a.int("x").zip(a.int("y"));
                let mode = a.str("mode").unwrap_or_else(|| "tap".into());
                let prefix = self.tap(&id, xy, a.str("text").as_deref(), &mode, 0)?;
                self.after_action(&id, a.observe(), &prefix)?
            }
            "type" => {
                self.type_text(&id, &a.req_str("text")?, a.flag("submit"))?;
                self.after_action(&id, a.observe(), "")?
            }
            "swipe" => {
                let d = Direction::parse(&a.req_str("direction")?)?;
                self.do_swipe(&id, d, a.int("x").zip(a.int("y")), a.int("distance"))?;
                self.after_action(&id, a.observe(), "")?
            }
            "key" => {
                self.do_key(&id, &a.req_str("button")?)?;
                self.after_action(&id, a.observe(), "")?
            }
            "app" => {
                let action = a.req_str("action")?;
                if action == "list" {
                    return Ok(Output::Text(self.list_apps(&id)?.join("\n")));
                }
                if action != "launch" && action != "stop" {
                    actionable!("action must be launch, stop or list");
                }
                self.app(&id, &action, a.str("package").as_deref())?;
                self.after_action(&id, a.observe(), "")?
            }
            "wait" => {
                let timeout = a
                    .int("timeout")
                    .map(|t| t.max(0) as u64)
                    .unwrap_or(DEFAULT_WAIT_MS);
                let s = self.wait_for(&id, &a.req_str("text")?, a.flag("gone"), timeout)?;
                self.report(&id, s)
            }
            "steps" => {
                let steps: Vec<String> = args
                    .get("steps")
                    .and_then(Value::as_array)
                    .map(|v| {
                        v.iter()
                            .filter_map(|s| s.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if steps.is_empty() || steps.len() > MAX_STEPS {
                    actionable!("steps takes 1 to {MAX_STEPS} items");
                }
                let mut notes = Vec::new();
                for (i, raw) in steps.iter().enumerate() {
                    let step = raw.trim();
                    if let Err(e) = self.step(&id, i, step, &mut notes) {
                        let reason = e.message.lines().next().unwrap_or("").to_string();
                        let screen = self.capture_screen(&id, false)?;
                        self.last_screen.insert(id.clone(), screen.clone());
                        notes.push(screen);
                        actionable!(
                            "step {} \"{step}\" failed: {reason}\n{}\n",
                            i + 1,
                            notes.join("\n")
                        );
                    }
                }
                let mut head = vec![format!("done {} steps", steps.len())];
                head.extend(notes);
                self.after_action(&id, true, &(head.join("\n") + "\n"))?
            }
            "net" => self.net(&id, &a)?,
            _ => return Err(Error::other(format!("unknown tool {name}"))),
        };
        Ok(Output::Text(text))
    }
}

/// tools/call: the MCP result object.
pub fn call(state: &Shared, name: &str, args: &Value) -> Value {
    log(&format!("Invoking {name} with args: {args}"));
    let result = state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .run(name, args);
    match result {
        Ok(Output::Text(t)) => json!({"content": [{"type": "text", "text": t}]}),
        Ok(Output::Image { data, mime }) => {
            json!({"content": [{"type": "image", "data": data, "mimeType": mime}]})
        }
        Err(e) if e.actionable => {
            json!({"content": [{"type": "text", "text": format!("{}. Please fix the issue and try again.", e.message)}], "isError": true})
        }
        Err(e) => {
            log(&format!("Tool '{name}' failed: {}", e.message));
            json!({"content": [{"type": "text", "text": format!("Error: {}", e.message)}], "isError": true})
        }
    }
}
