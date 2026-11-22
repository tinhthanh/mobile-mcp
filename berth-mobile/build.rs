//! Builds the Apple Vision OCR helper (../native/ocr.swift) into the binary
//! on macOS, so nothing is compiled or fetched on the user's computer.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_ocr)");
    println!("cargo::rerun-if-changed=../native/ocr.swift");
    println!("cargo::rerun-if-env-changed=BERTH_MOBILE_NO_OCR");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os != "macos" || std::env::var_os("BERTH_MOBILE_NO_OCR").is_some() {
        return;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => return,
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ocr");
    let status = Command::new("xcrun")
        .args([
            "swiftc",
            "-O",
            "-target",
            &format!("{arch}-apple-macos13"),
            "../native/ocr.swift",
            "-o",
        ])
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo::rustc-cfg=has_ocr"),
        _ => println!("cargo::warning=swiftc failed: building without OCR"),
    }
}
