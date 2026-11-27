//! iPhone Mirroring (macOS 15+): a fallback for an iPhone that can't run
//! WebDriverAgent here (not in the signing profile, no cable). The phone's
//! screen is the iPhone Mirroring window on this Mac: it is captured with
//! `screencapture`, read with the built-in Apple Vision OCR, and driven with
//! HID-level mouse and keyboard events, like a person at the Mac.
//!
//! Only macOS's own C APIs (CoreGraphics, CoreFoundation, ApplicationServices)
//! and tools (`screencapture`, `lsappinfo`, `open`): no extra crates. HID
//! events move the real pointer, so this takes the Mac's mouse while it acts
//! (it puts the pointer back after each action); hence off by default.

use std::ffi::c_void;
use std::process::Command;
use std::time::Duration;

use crate::adb::Direction;
use crate::util::{Error, Result, run, sleep_ms};

pub const DEVICE_ID: &str = "mirroring";
const BUNDLE_ID: &str = "com.apple.ScreenContinuity";

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

type Ref = *const c_void;
type MutRef = *mut c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
    fn CFNumberGetValue(number: Ref, the_type: isize, value: *mut c_void) -> bool;
    fn CFRelease(cf: Ref);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGWindowNumber: Ref;
    static kCGWindowOwnerPID: Ref;
    static kCGWindowBounds: Ref;
    static kCGWindowLayer: Ref;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Ref;
    fn CGRectMakeWithDictionaryRepresentation(dict: Ref, rect: *mut CGRect) -> bool;
    fn CGEventSourceCreate(state: i32) -> MutRef;
    fn CGEventCreate(source: MutRef) -> MutRef;
    fn CGEventGetLocation(event: MutRef) -> CGPoint;
    fn CGEventCreateMouseEvent(source: MutRef, kind: u32, at: CGPoint, button: u32) -> MutRef;
    fn CGEventCreateKeyboardEvent(source: MutRef, keycode: u16, down: bool) -> MutRef;
    fn CGEventKeyboardSetUnicodeString(event: MutRef, length: usize, chars: *const u16);
    fn CGEventSetFlags(event: MutRef, flags: u64);
    fn CGEventSetIntegerValueField(event: MutRef, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: MutRef);
    fn CGWarpMouseCursorPosition(to: CGPoint) -> i32;
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

const HID_TAP: u32 = 0; // kCGHIDEventTap
const HID_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState
const MOUSE_DOWN: u32 = 1;
const MOUSE_UP: u32 = 2;
const MOUSE_DRAGGED: u32 = 6;
const CLICK_STATE: u32 = 1; // kCGMouseEventClickState
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_COMMAND: u64 = 1 << 20;
const NUMBER_I64: isize = 4; // kCFNumberSInt64Type

/// An owned CoreGraphics/CoreFoundation object, released on drop.
struct Owned(MutRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// The iPhone Mirroring window: its id and frame in global points.
#[derive(Clone, Copy, Debug)]
struct Window {
    id: u32,
    frame: CGRect,
}

fn number(dict: Ref, key: Ref) -> Option<i64> {
    let v = unsafe { CFDictionaryGetValue(dict, key) };
    if v.is_null() {
        return None;
    }
    let mut out: i64 = 0;
    unsafe { CFNumberGetValue(v, NUMBER_I64, (&mut out as *mut i64).cast()) }.then_some(out)
}

/// iPhone Mirroring's pid, when it runs.
fn pid() -> Option<i64> {
    let mut cmd = Command::new("/usr/bin/lsappinfo");
    cmd.args(["info", "-only", "pid", "-app", BUNDLE_ID]);
    let out = String::from_utf8_lossy(&run(cmd, Duration::from_secs(5), true).ok()?).into_owned();
    // "pid"=12345
    out.split('=').nth(1)?.trim().parse().ok()
}

/// The app's main window: layer 0, the biggest one it owns.
fn window() -> Option<Window> {
    let pid = pid()?;
    let list = Owned(unsafe { CGWindowListCopyWindowInfo(0, 0) } as MutRef);
    if list.0.is_null() {
        return None;
    }
    let mut best: Option<Window> = None;
    for i in 0..unsafe { CFArrayGetCount(list.0) } {
        let w = unsafe { CFArrayGetValueAtIndex(list.0, i) };
        if number(w, unsafe { kCGWindowOwnerPID }) != Some(pid)
            || number(w, unsafe { kCGWindowLayer }) != Some(0)
        {
            continue;
        }
        let mut frame = CGRect::default();
        let bounds = unsafe { CFDictionaryGetValue(w, kCGWindowBounds) };
        if bounds.is_null()
            || !unsafe { CGRectMakeWithDictionaryRepresentation(bounds, &mut frame) }
        {
            continue;
        }
        if frame.size.width < 100.0 || frame.size.height < 200.0 {
            continue;
        }
        let id = number(w, unsafe { kCGWindowNumber }).unwrap_or(0) as u32;
        let area = |f: &CGRect| f.size.width * f.size.height;
        if best.is_none_or(|b| area(&frame) > area(&b.frame)) {
            best = Some(Window { id, frame });
        }
    }
    best
}

/// What stops mirroring from being used right now, or None when it can be.
pub fn problem() -> Option<String> {
    if !crate::settings::get(crate::settings::MIRRORING) {
        return Some(
            "iPhone Mirroring mode is off (Shipyard → Settings → Plugins → Mobile)".into(),
        );
    }
    if !unsafe { AXIsProcessTrusted() } {
        return Some(
            "macOS hasn't allowed this app to control the computer: System Settings → Privacy & Security → Accessibility"
                .into(),
        );
    }
    if !unsafe { CGPreflightScreenCaptureAccess() } {
        return Some(
            "macOS hasn't allowed this app to record the screen: System Settings → Privacy & Security → Screen & System Audio Recording"
                .into(),
        );
    }
    if window().is_none() {
        return Some("open iPhone Mirroring on this Mac and connect the iPhone (it must be locked and nearby)".into());
    }
    None
}

/// Whether the switch is on (the device is then listed, usable or not).
pub fn enabled() -> bool {
    crate::settings::get(crate::settings::MIRRORING)
}

fn usable() -> Result<Window> {
    if let Some(p) = problem() {
        return Err(Error::actionable(format!("iPhone Mirroring: {p}")));
    }
    window().ok_or_else(|| Error::actionable("iPhone Mirroring: its window is gone"))
}

/// Brings iPhone Mirroring to the front, so keys reach it.
fn activate() {
    let mut cmd = Command::new("/usr/bin/open");
    cmd.args(["-b", BUNDLE_ID]);
    let _ = run(cmd, Duration::from_secs(5), true);
    sleep_ms(150);
}

/// The window as PNG and how many pixels make a point.
pub fn screenshot() -> Result<(Vec<u8>, f64)> {
    let w = usable()?;
    let file = std::env::temp_dir().join(format!("berth-mobile-mirror-{}.png", std::process::id()));
    let mut cmd = Command::new("/usr/sbin/screencapture");
    cmd.args(["-x", "-o", "-l", &w.id.to_string()]).arg(&file);
    run(cmd, Duration::from_secs(15), false)?;
    let png = std::fs::read(&file)?;
    let _ = std::fs::remove_file(&file);
    let px = png
        .get(16..20)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0) as f64;
    let scale = if px > 0.0 && w.frame.size.width > 0.0 {
        px / w.frame.size.width
    } else {
        1.0
    };
    Ok((png, scale))
}

/// The compact screen: OCR lines, in points from the window's top left.
pub fn screen_lines() -> Result<Vec<String>> {
    let (png, scale) = screenshot()?;
    let lines = crate::ocr::ocr_lines(&png)
        .ok_or_else(|| Error::other("OCR is not available in this build"))?;
    Ok(lines
        .into_iter()
        .filter_map(|l| {
            let (text, xy) = l.rsplit_once('|')?;
            let (x, y) = xy.split_once(',')?;
            let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?);
            // OCR can't tell a button from a label: mark every line tappable
            Some(format!(
                "*{}|{},{}",
                text.trim_start(),
                (x / scale) as i64,
                (y / scale) as i64
            ))
        })
        .collect())
}

/// Runs `f` with HID events, then puts the pointer back where the person had it.
fn with_pointer<T>(f: impl FnOnce(MutRef) -> T) -> T {
    let src = Owned(unsafe { CGEventSourceCreate(HID_STATE) });
    let here = Owned(unsafe { CGEventCreate(std::ptr::null_mut()) });
    let back = unsafe { CGEventGetLocation(here.0) };
    let out = f(src.0);
    unsafe { CGWarpMouseCursorPosition(back) };
    out
}

fn post_mouse(src: MutRef, kind: u32, at: CGPoint, clicks: i64) {
    let e = Owned(unsafe { CGEventCreateMouseEvent(src, kind, at, 0) });
    if clicks > 0 {
        unsafe { CGEventSetIntegerValueField(e.0, CLICK_STATE, clicks) };
    }
    unsafe { CGEventPost(HID_TAP, e.0) };
}

fn global(w: &Window, x: i64, y: i64) -> CGPoint {
    CGPoint {
        x: w.frame.origin.x + x as f64,
        y: w.frame.origin.y + y as f64,
    }
}

/// Tap at window point (x, y): `hold_ms` long, `count` times.
pub fn tap(x: i64, y: i64, hold_ms: u64, count: i64) -> Result<()> {
    let w = usable()?;
    let at = global(&w, x, y);
    with_pointer(|src| {
        for n in 1..=count {
            post_mouse(src, MOUSE_DOWN, at, n);
            sleep_ms(hold_ms);
            post_mouse(src, MOUSE_UP, at, n);
            sleep_ms(60);
        }
    });
    Ok(())
}

fn drag(w: &Window, from: CGPoint, to: CGPoint) {
    with_pointer(|src| {
        post_mouse(src, MOUSE_DOWN, from, 1);
        sleep_ms(60);
        for i in 1..=12 {
            let t = i as f64 / 12.0;
            let p = CGPoint {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            };
            post_mouse(src, MOUSE_DRAGGED, p, 0);
            sleep_ms(25);
        }
        post_mouse(src, MOUSE_UP, to, 1);
    });
    let _ = w;
}

pub fn swipe(d: Direction, from: Option<(i64, i64)>, distance: Option<i64>) -> Result<()> {
    let w = usable()?;
    let (ww, wh) = (w.frame.size.width, w.frame.size.height);
    let (x0, y0) = from.map(|(x, y)| (x as f64, y as f64)).unwrap_or(match d {
        Direction::Up => (ww / 2.0, wh * 0.75),
        Direction::Down => (ww / 2.0, wh * 0.25),
        Direction::Left => (ww * 0.8, wh / 2.0),
        Direction::Right => (ww * 0.2, wh / 2.0),
    });
    let dy = distance.map(|v| v as f64).unwrap_or(wh * 0.45);
    let dx = distance.map(|v| v as f64).unwrap_or(ww * 0.6);
    let (x1, y1) = match d {
        Direction::Up => (x0, (y0 - dy).max(1.0)),
        Direction::Down => (x0, (y0 + dy).min(wh - 1.0)),
        Direction::Left => ((x0 - dx).max(1.0), y0),
        Direction::Right => ((x0 + dx).min(ww - 1.0), y0),
    };
    let o = w.frame.origin;
    drag(
        &w,
        CGPoint {
            x: o.x + x0,
            y: o.y + y0,
        },
        CGPoint {
            x: o.x + x1,
            y: o.y + y1,
        },
    );
    Ok(())
}

/// US keyboard key codes for printable ASCII: (code, needs shift).
fn key_for(c: char) -> Option<(u16, bool)> {
    const PLAIN: &str = "asdfhgzxcv\u{0}bqweryt123465=97-80]ou[ip\u{0}lj'k;\\,/nm.\u{0}\u{0}`";
    const SHIFTED: &str = "ASDFHGZXCV\u{0}BQWERYT!@#$^%+(&_*)}OU{IP\u{0}LJ\"K:|<?NM>\u{0}\u{0}~";
    if c == ' ' {
        return Some((49, false));
    }
    if let Some(i) = PLAIN.chars().position(|p| p == c && p != '\u{0}') {
        return Some((i as u16, false));
    }
    SHIFTED
        .chars()
        .position(|p| p == c && p != '\u{0}')
        .map(|i| (i as u16, true))
}

fn post_key(src: MutRef, code: u16, flags: u64) {
    for down in [true, false] {
        let e = Owned(unsafe { CGEventCreateKeyboardEvent(src, code, down) });
        unsafe { CGEventSetFlags(e.0, flags) };
        unsafe { CGEventPost(HID_TAP, e.0) };
        sleep_ms(12);
    }
}

fn post_unicode(src: MutRef, c: char) {
    let mut buf = [0u16; 2];
    let units = c.encode_utf16(&mut buf);
    for down in [true, false] {
        let e = Owned(unsafe { CGEventCreateKeyboardEvent(src, 0, down) });
        unsafe { CGEventKeyboardSetUnicodeString(e.0, units.len(), units.as_ptr()) };
        unsafe { CGEventPost(HID_TAP, e.0) };
        sleep_ms(12);
    }
}

/// Types at the iPhone's cursor (it appends; tap the field first).
pub fn type_text(text: &str) -> Result<()> {
    usable()?;
    activate();
    with_pointer(|src| {
        for c in text.chars() {
            match key_for(c) {
                Some((code, shift)) => post_key(src, code, if shift { FLAG_SHIFT } else { 0 }),
                None => post_unicode(src, c),
            }
        }
    });
    Ok(())
}

/// HOME (⌘1), ENTER; the rest has no key in iPhone Mirroring.
pub fn press_button(button: &str) -> Result<()> {
    usable()?;
    let (code, flags) = match button.to_ascii_uppercase().as_str() {
        "HOME" => (18, FLAG_COMMAND),
        "ENTER" => (36, 0),
        other => {
            return Err(Error::actionable(format!(
                "{other} isn't available through iPhone Mirroring: use HOME or ENTER, or tap the app's own buttons"
            )));
        }
    };
    activate();
    with_pointer(|src| post_key(src, code, flags));
    Ok(())
}

/// Opens an app by its name through the iPhone's Spotlight (⌘3).
pub fn launch(name: &str) -> Result<()> {
    usable()?;
    activate();
    with_pointer(|src| post_key(src, 20, FLAG_COMMAND));
    sleep_ms(700);
    type_text(name)?;
    sleep_ms(600);
    with_pointer(|src| post_key(src, 36, 0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::key_for;

    #[test]
    fn maps_us_keys() {
        assert_eq!(key_for('a'), Some((0, false)));
        assert_eq!(key_for('A'), Some((0, true)));
        assert_eq!(key_for('1'), Some((18, false)));
        assert_eq!(key_for('!'), Some((18, true)));
        assert_eq!(key_for('@'), Some((19, true)));
        assert_eq!(key_for('m'), Some((46, false)));
        assert_eq!(key_for('.'), Some((47, false)));
        assert_eq!(key_for('?'), Some((44, true)));
        assert_eq!(key_for(' '), Some((49, false)));
        assert_eq!(key_for('ế'), None);
    }
}
