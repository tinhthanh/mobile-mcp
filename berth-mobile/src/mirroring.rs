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
use std::sync::atomic::{AtomicU32, Ordering};
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
    static kCGWindowIsOnscreen: Ref;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> Ref;
    fn CGRectMakeWithDictionaryRepresentation(dict: Ref, rect: *mut CGRect) -> bool;
    fn CGEventSourceCreate(state: i32) -> MutRef;
    fn CGEventCreate(source: MutRef) -> MutRef;
    fn CGEventGetLocation(event: MutRef) -> CGPoint;
    fn CGEventCreateMouseEvent(source: MutRef, kind: u32, at: CGPoint, button: u32) -> MutRef;
    fn CGEventCreateKeyboardEvent(source: MutRef, keycode: u16, down: bool) -> MutRef;
    fn CGEventSetFlags(event: MutRef, flags: u64);
    fn CGEventSetIntegerValueField(event: MutRef, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: MutRef);
    fn CGWarpMouseCursorPosition(to: CGPoint) -> i32;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGEventCreateScrollWheelEvent2(
        source: MutRef,
        units: u32,
        count: u32,
        w1: i32,
        w2: i32,
        w3: i32,
    ) -> MutRef;
    fn CGEventSetLocation(event: MutRef, at: CGPoint);
    fn CGEventSetDoubleValueField(event: MutRef, field: u32, value: f64);
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const std::ffi::c_char) -> MutRef;
    fn IOServiceGetMatchingService(main_port: u32, matching: MutRef) -> u32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

const HID_TAP: u32 = 0; // kCGHIDEventTap
const HID_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState
const MOUSE_DOWN: u32 = 1;
const MOUSE_UP: u32 = 2;
const MOUSE_MOVED: u32 = 5;
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
    pid: i32,
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

/// The phone's window: on screen, layer 0, shaped like a phone (portrait);
/// the app also keeps other windows, such as an empty "Welcome" one.
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
        let onscreen = !unsafe { CFDictionaryGetValue(w, kCGWindowIsOnscreen) }.is_null()
            && number(w, unsafe { kCGWindowIsOnscreen }).unwrap_or(0) != 0;
        let portrait = frame.size.height > frame.size.width * 1.5;
        if std::env::var_os("BERTH_MOBILE_DEBUG_WINDOWS").is_some() {
            crate::util::log(&format!(
                "mirroring window {id}: {frame:?} onscreen={onscreen} portrait={portrait}"
            ));
        }
        if !onscreen || !portrait {
            continue;
        }
        let area = |f: &CGRect| f.size.width * f.size.height;
        if best.is_none_or(|b| area(&frame) > area(&b.frame)) {
            best = Some(Window {
                id,
                pid: pid as i32,
                frame,
            });
        }
    }
    best
}

/// What stops mirroring from being used right now, or None when it can be.
pub fn problem() -> Option<String> {
    if let Some(p) = problem_without_window() {
        return Some(p);
    }
    if window().is_none() {
        return Some("open iPhone Mirroring on this Mac and connect the iPhone (it must be locked and nearby)".into());
    }
    None
}

fn problem_without_window() -> Option<String> {
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
    None
}

/// Whether the switch is on (the device is then listed, usable or not).
pub fn enabled() -> bool {
    crate::settings::get(crate::settings::MIRRORING)
}

fn usable() -> Result<Window> {
    // the window drops out of the on-screen list for a moment now and then
    // (mid-animation), or sits behind another Space: bring it forward once and
    // give it a second and a half before calling it gone
    for i in 0..6 {
        if let Some(w) = window() {
            if let Some(p) = problem_without_window() {
                return Err(Error::actionable(format!("iPhone Mirroring: {p}")));
            }
            return Ok(w);
        }
        if i == 1 && pid().is_some() && crate::settings::get(crate::settings::MIRRORING) {
            activate();
        }
        sleep_ms(250);
    }
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
    // the window can be mid-redraw right after an action: try again
    let shot = || {
        let mut cmd = Command::new("/usr/sbin/screencapture");
        cmd.args(["-x", "-o", "-l", &w.id.to_string()]).arg(&file);
        run(cmd, Duration::from_secs(15), false)
    };
    let mut tries = 0;
    while let Err(e) = shot() {
        tries += 1;
        if tries == 3 {
            return Err(e);
        }
        sleep_ms(400);
    }
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
    // iPhone Mirroring handles the events a moment later and reads where the
    // pointer is then: put it back only after that
    sleep_ms(250);
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

/// Mouse-button records addressed to a window, through SkyLight's
/// SLPSPostEventRecordTo (what the window server uses to hand a process an
/// event for one of its windows): taps land without moving the person's
/// pointer or bringing iPhone Mirroring forward. The record layout is
/// jfarcand/mirroir-mcp's (Apache-2.0), measured on macOS 26. Private API,
/// looked up at run time: None when this macOS lacks it.
mod sky {
    use std::ffi::{CStr, c_void};
    use std::sync::OnceLock;

    type Post = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
    type ProcessForPid = unsafe extern "C" fn(i32, *mut c_void) -> i32;

    struct Api {
        post: Post,
        psn: ProcessForPid,
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let open = |p: &CStr| libc::dlopen(p.as_ptr(), libc::RTLD_NOW);
            let sky = open(c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight");
            let svc = open(
                c"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices",
            );
            if sky.is_null() || svc.is_null() {
                return None;
            }
            let post = libc::dlsym(sky, c"SLPSPostEventRecordTo".as_ptr());
            let psn = libc::dlsym(svc, c"GetProcessForPID".as_ptr());
            if post.is_null() || psn.is_null() {
                return None;
            }
            Some(Api {
                post: std::mem::transmute::<*mut c_void, Post>(post),
                psn: std::mem::transmute::<*mut c_void, ProcessForPid>(psn),
            })
        })
        .as_ref()
    }

    pub fn available() -> bool {
        api().is_some()
    }

    /// A press (`down`) or release at window point `at`; `origin` is the
    /// window's top left in global points.
    pub fn button(
        window: u32,
        pid: i32,
        origin: (f64, f64),
        at: (f64, f64),
        down: bool,
        clicks: i32,
        number: i16,
    ) -> bool {
        let Some(api) = api() else { return false };
        let (x, y) = at;
        let mut r = [0u8; 0xf8];
        r[0x04] = 0xf8; // length
        r[0x08] = if down { 1 } else { 2 }; // mouse down / up
        r[0x10..0x18].copy_from_slice(&(origin.0 + x).to_ne_bytes());
        r[0x18..0x20].copy_from_slice(&(origin.1 + y).to_ne_bytes());
        r[0x20..0x28].copy_from_slice(&x.to_ne_bytes());
        r[0x28..0x30].copy_from_slice(&y.to_ne_bytes());
        r[0x3a] = 0x10; // addressed to the window below
        r[0x3c..0x40].copy_from_slice(&window.to_ne_bytes());
        r[0x8a..0x8c].copy_from_slice(&number.to_ne_bytes());
        r[0x8c..0x90].copy_from_slice(&clicks.to_ne_bytes());
        r[0x90] = if down { 0xff } else { 0 }; // pressure
        let mut psn = [0u32; 2];
        unsafe {
            (api.psn)(pid, psn.as_mut_ptr().cast()) == 0
                && (api.post)(psn.as_mut_ptr().cast(), r.as_mut_ptr().cast()) == 0
        }
    }
}

/// Tap at window point (x, y): `hold_ms` long, `count` times.
pub fn tap(x: i64, y: i64, hold_ms: u64, count: i64) -> Result<()> {
    let w = usable()?;
    let at = global(&w, x, y);
    if std::env::var_os("BERTH_MOBILE_DEBUG_WINDOWS").is_some() {
        crate::util::log(&format!(
            "mirroring tap ({x},{y}) in {:?} → {at:?} (window records: {})",
            w.frame,
            sky::available()
        ));
    }
    if sky::available() {
        // every press gets a fresh event number, shared with its release
        static NUMBER: AtomicU32 = AtomicU32::new(0);
        let origin = (w.frame.origin.x, w.frame.origin.y);
        let at = (x as f64, y as f64);
        let mut sent = true;
        for n in 1..=count {
            let seed = std::process::id() & 0x3fff;
            let number = ((seed + NUMBER.fetch_add(1, Ordering::Relaxed) + 1) & 0x7fff) as i16;
            sent = sky::button(w.id, w.pid, origin, at, true, n as i32, number);
            if !sent {
                break;
            }
            sleep_ms(hold_ms);
            sky::button(w.id, w.pid, origin, at, false, n as i32, number);
            sleep_ms(60);
        }
        if sent {
            return Ok(());
        }
    }
    // a click on a window that isn't frontmost only brings it forward
    activate();
    with_pointer(|src| {
        post_mouse(src, MOUSE_MOVED, at, 0);
        sleep_ms(40);
        for n in 1..=count {
            post_mouse(src, MOUSE_DOWN, at, n);
            sleep_ms(hold_ms);
            post_mouse(src, MOUSE_UP, at, n);
            sleep_ms(60);
        }
    });
    Ok(())
}

/// The registry id of a real input device: iPhone Mirroring trusts scroll
/// gestures that name one.
fn sender_id() -> Option<u64> {
    for class in [
        c"AppleMultitouchDevice",
        c"AppleHIDKeyboardEventDriverV2",
        c"IOHIDEventService",
    ] {
        let service = unsafe { IOServiceGetMatchingService(0, IOServiceMatching(class.as_ptr())) };
        if service == 0 {
            continue;
        }
        let mut id = 0u64;
        let ok = unsafe { IORegistryEntryGetRegistryEntryID(service, &mut id) } == 0;
        unsafe { IOObjectRelease(service) };
        if ok {
            return Some(id);
        }
    }
    None
}

/// One continuous (trackpad) scroll frame of (dy, dx) points at `at`.
/// Field numbers and scaling are jfarcand/mirroir-mcp's (Apache-2.0),
/// measured against iPhone Mirroring: it ignores bare wheel events.
fn post_scroll(at: CGPoint, d: (i32, i32), phase: i64, sender: Option<u64>) {
    const PER_LINE: f64 = 10.2;
    let line = |p: i32| -> i64 {
        let whole = (p as f64 / PER_LINE).trunc() as i64;
        if whole == 0 && p.abs() >= 5 {
            p.signum() as i64
        } else {
            whole
        }
    };
    let e =
        Owned(unsafe { CGEventCreateScrollWheelEvent2(std::ptr::null_mut(), 0, 2, d.0, d.1, 0) });
    if e.0.is_null() {
        return;
    }
    unsafe {
        CGEventSetLocation(e.0, at);
        CGEventSetFlags(e.0, 0);
        let set = |f: u32, v: i64| CGEventSetIntegerValueField(e.0, f, v);
        set(88, 1); // continuous
        set(45, 1); // gesture
        set(137, 1); // from a trackpad
        if let Some(id) = sender {
            set(87, id as i64);
        }
        set(169, mach_absolute_time() as i64);
        set(173, 0);
        set(11, line(d.0));
        set(12, line(d.1));
        set(93, (d.0 as f64 / PER_LINE * 65536.0) as i64);
        set(94, (d.1 as f64 / PER_LINE * 65536.0) as i64);
        CGEventSetDoubleValueField(e.0, 176, -(d.0 as f64) / PER_LINE);
        CGEventSetDoubleValueField(e.0, 175, -(d.1 as f64) / PER_LINE);
        CGEventSetDoubleValueField(e.0, 178, -(d.0 as f64) * 1.75);
        CGEventSetDoubleValueField(e.0, 177, -(d.1 as f64) * 1.75);
        // point deltas last: the line deltas above recompute them
        set(96, d.0 as i64);
        set(97, d.1 as i64);
        set(99, phase); // scroll phase
        set(123, 0); // no momentum
        CGEventPost(HID_TAP, e.0);
    }
}

/// A finger drag of (dx, dy) points through the middle of `from`..`to`, as a
/// trackpad scroll gesture: iPhone Mirroring turns it into a swipe (it
/// ignores mouse drags). Slow and momentum-free, so lists move one to one.
fn scroll_swipe(from: CGPoint, to: CGPoint) {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let mid = CGPoint {
        x: from.x + dx / 2.0,
        y: from.y + dy / 2.0,
    };
    let distance = (dx * dx + dy * dy).sqrt();
    let frames = ((distance / 140.0 * 1000.0 / 16.0).ceil() as usize).clamp(5, 600);
    let total = ((dy * 1.6) as i32, (dx * 1.6) as i32);
    let sender = sender_id();
    with_pointer(|src| {
        // scroll goes where the pointer is: put it in the window, hold it there
        unsafe { CGWarpMouseCursorPosition(mid) };
        unsafe { CGAssociateMouseAndMouseCursorPosition(false) };
        post_mouse(src, MOUSE_MOVED, mid, 0);
        sleep_ms(50);
        post_scroll(mid, (0, 0), 128, sender); // may begin
        sleep_ms(100);
        let mut sent = (0i32, 0i32);
        for i in 1..=frames {
            let want = (
                (total.0 as i64 * i as i64 / frames as i64) as i32,
                (total.1 as i64 * i as i64 / frames as i64) as i32,
            );
            post_scroll(
                mid,
                (want.0 - sent.0, want.1 - sent.1),
                if i == 1 { 1 } else { 2 },
                sender,
            );
            sent = want;
            sleep_ms(16);
        }
        post_scroll(mid, (0, 0), 4, sender); // lift
        sleep_ms(16);
        unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    });
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
    scroll_swipe(
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

fn post_key(src: MutRef, code: u16, flags: u64) {
    // a chord presses its modifier keys first, as a person does: the iPhone
    // drops a ⌘ that only rides on the letter's flags (⌘V arrives as "v")
    let mods: Vec<(u64, u16)> = [(FLAG_COMMAND, 55u16), (FLAG_SHIFT, 56)]
        .into_iter()
        .filter(|(f, _)| flags & f != 0)
        .collect();
    let key = |code: u16, down: bool, flags: u64| {
        let e = Owned(unsafe { CGEventCreateKeyboardEvent(src, code, down) });
        unsafe { CGEventSetFlags(e.0, flags) };
        unsafe { CGEventPost(HID_TAP, e.0) };
        sleep_ms(12);
    };
    let mut held = 0;
    for &(f, c) in &mods {
        held |= f;
        key(c, true, held);
    }
    key(code, true, flags);
    key(code, false, flags);
    for &(f, c) in mods.iter().rev() {
        held &= !f;
        key(c, false, held);
    }
    if !mods.is_empty() {
        sleep_ms(15);
    }
}

/// The Mac's clipboard as text (what pbpaste gives; other kinds read as None).
fn clipboard() -> Option<Vec<u8>> {
    Command::new("/usr/bin/pbpaste")
        .env("LANG", "en_US.UTF-8")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| o.stdout)
}

fn set_clipboard(bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("/usr/bin/pbcopy")
        .env("LANG", "en_US.UTF-8")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| Error::other("pbcopy: no stdin"))?
        .write_all(bytes)?;
    child.wait()?;
    Ok(())
}

/// Types at the iPhone's cursor (it appends; tap the field first) by pasting
/// through the clipboard iPhone Mirroring shares: key by key, the iPhone's
/// own keyboard layout rewrites the text (Vietnamese Telex turns "Safari"
/// into "Sàải"). The Mac's clipboard text is put back afterwards.
pub fn type_text(text: &str) -> Result<()> {
    usable()?;
    activate();
    let saved = clipboard();
    set_clipboard(text.as_bytes())?;
    // the iPhone picks the clipboard up over Continuity: ⌘V sooner pastes
    // what it held before
    sleep_ms(1500);
    with_pointer(|src| post_key(src, 9, FLAG_COMMAND));
    sleep_ms(1000);
    set_clipboard(saved.as_deref().unwrap_or_default())?;
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

/// Searches the iPhone's Spotlight (⌘3) for an app by name and leaves the
/// results up: the agent taps the app's own result.
pub fn launch(name: &str) -> Result<()> {
    usable()?;
    activate();
    with_pointer(|src| post_key(src, 20, FLAG_COMMAND));
    sleep_ms(700);
    // Spotlight can come back holding the last query: select it, delete it
    with_pointer(|src| {
        post_key(src, 0, FLAG_COMMAND);
        sleep_ms(80);
        post_key(src, 51, 0);
    });
    sleep_ms(200);
    type_text(name)?;
    sleep_ms(900);
    // Return would open Spotlight's top hit, which can be a message or a web
    // suggestion: leave the results up for the agent to tap the app's own
    Ok(())
}
