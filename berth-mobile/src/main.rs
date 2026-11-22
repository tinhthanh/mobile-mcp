//! berth-mobile: drive Android phones over adb from coding agents.
//!
//!   berth-mobile [mcp]          MCP server on stdio (what agents run)
//!   berth-mobile setup          fetch adb if needed, install UiAutomator2 on connected phones
//!   berth-mobile status --json  whether setup is done, for Shipyard
//!   berth-mobile settings --json  the switches Shipyard shows
//!   berth-mobile version

mod adb;
mod fetch;
mod lock;
mod mcp;
mod net;
mod ocr;
mod paths;
mod pcap;
mod screen;
mod settings;
mod shot;
mod tools;
mod uia2;
mod util;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("mcp") {
        "mcp" | "--stdio" => serve(),
        "setup" => std::process::exit(setup()),
        "status" => println!("{}", status()),
        "settings" => println!("{}", settings::list_json()),
        "version" | "--version" | "-V" => println!("berth-mobile {}", env!("CARGO_PKG_VERSION")),
        other => {
            eprintln!("unknown command {other:?}; see the top of src/main.rs");
            std::process::exit(2);
        }
    }
}

static SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    let fd = SIGNAL_PIPE.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        let b = [1u8];
        unsafe { libc::write(fd, b.as_ptr().cast(), 1) };
    }
}

/// Runs `cleanup` when the client stops the server with a signal. A handler
/// (not a blocked mask) so that adb and mitmdump started from here keep the
/// default signal behaviour.
fn on_termination(cleanup: impl FnOnce() + Send + 'static) {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    SIGNAL_PIPE.store(fds[1], std::sync::atomic::Ordering::Relaxed);
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) };
    }
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        if unsafe { libc::read(fds[0], b.as_mut_ptr().cast(), 1) } == 1 {
            cleanup();
            std::process::exit(0);
        }
    });
}

fn serve() {
    let state: tools::Shared = Arc::new(Mutex::new(tools::State::default()));
    let s = state.clone();
    on_termination(move || tools::shutdown(&s));
    tools::spawn_idle_reaper(state.clone());
    mcp::serve(state.clone());
    tools::shutdown(&state);
}

fn status() -> Value {
    let adb = adb::find_adb(false);
    let version = adb.as_deref().and_then(adb::adb_version);
    let pinned = version.as_deref() == Some(adb::ADB_VERSION)
        || std::env::var_os("BERTH_MOBILE_ADB").is_some()
        || (adb.is_some() && adb::PLATFORM_TOOLS.is_none());
    let mut phones = Vec::new();
    if adb.is_some()
        && let Ok(list) = adb::list_devices()
    {
        for d in list {
            let mut p = json!({"serial": d.serial, "state": d.state});
            if d.state == "device" {
                p["uia2"] = json!(uia2::installed(&adb::Device::new(&d.serial)));
            }
            phones.push(p);
        }
    }
    let problem = match (&adb, pinned) {
        (None, _) => Some("Chưa có adb: bấm Set up… để tải platform-tools của Google"),
        (Some(_), false) => Some(
            "adb trên máy này khác bản 37.0.1 mà các plugin dùng chung: bấm Set up… để tải bản đúng (tránh hai adb giành nhau)",
        ),
        _ => None,
    };
    json!({
        "ready": problem.is_none(),
        "problem": problem,
        "adb": adb.map(|p| p.display().to_string()),
        "adbVersion": version,
        "phones": phones,
        "settings": settings::list_json()["settings"],
    })
}

/// Headless setup, started by Shipyard when the person allows the plugin or
/// clicks Set up…; its log goes to the state folder.
fn setup() -> i32 {
    let log_path = paths::state_dir().join("setup.log");
    let _ = paths::ensure_private(&paths::state_dir());
    if std::env::var_os("LOG_FILE").is_none() {
        unsafe { std::env::set_var("LOG_FILE", &log_path) };
    }
    let adb = match adb::find_adb(false) {
        Some(p) if adb::adb_version(&p).as_deref() == Some(adb::ADB_VERSION) => p,
        _ => match adb::fetch_platform_tools() {
            Ok(p) => p,
            Err(e) => {
                util::log(&format!("setup: couldn't fetch platform-tools: {e}"));
                match adb::find_adb(false) {
                    Some(p) => p,
                    None => return 1,
                }
            }
        },
    };
    util::log(&format!("setup: adb {}", adb.display()));
    let mut failed = false;
    match adb::ready_devices() {
        Ok(serials) => {
            for s in serials {
                let dev = adb::Device::new(&s);
                if uia2::installed(&dev) {
                    util::log(&format!(
                        "setup: {s}: UiAutomator2 {} already installed",
                        uia2::VERSION
                    ));
                } else if let Err(e) = uia2::install(&dev) {
                    util::log(&format!("setup: {s}: {e}"));
                    failed = true;
                } else {
                    util::log(&format!(
                        "setup: {s}: UiAutomator2 {} installed",
                        uia2::VERSION
                    ));
                }
            }
        }
        Err(e) => util::log(&format!("setup: adb devices failed: {e}")),
    }
    if failed { 1 } else { 0 }
}
