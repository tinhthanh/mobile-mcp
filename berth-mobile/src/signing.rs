//! Signing WebDriverAgent for an iPhone. iOS runs nothing that isn't signed
//! for the device, so the person's Apple Developer team provides, once, in
//! the state folder's `signing/`:
//!
//! - `berth-mobile.keychain-db` + `keychain.pass`: an Apple Development
//!   identity (key made on this Mac) in a keychain of its own;
//! - `wda.mobileprovision`: a development profile for a wildcard App ID
//!   (`TEAM.prefix.*`) that lists the iPhones.
//!
//! Appium's unsigned WebDriverAgent build (pinned) is re-signed with them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::fetch::Pinned;
use crate::util::{Error, Result, log, run, sha256_hex};

pub const WDA: Pinned = Pinned {
    url: "https://github.com/appium/WebDriverAgent/releases/download/v16.15.3/WebDriverAgentRunner-Runner.zip",
    sha256: "2176793538d902f6b1e61291e9997f68d306541f222ea715d256a7c6ce9d4fc5",
};

pub fn dir() -> PathBuf {
    crate::paths::state_dir().join("signing")
}

fn keychain() -> PathBuf {
    dir().join("berth-mobile.keychain-db")
}

fn profile_path() -> PathBuf {
    dir().join("wda.mobileprovision")
}

/// What the profile says.
pub struct Profile {
    pub team: String,
    /// "vn.berthmobile" for `TEAM.vn.berthmobile.*`
    pub prefix: String,
    pub devices: Vec<String>,
    pub expires: String,
    pub sha: String,
}

impl Profile {
    pub fn runner_bundle(&self) -> String {
        format!("{}.WebDriverAgentRunner.xctrunner", self.prefix)
    }
}

fn plist_value(plist: &Path, key: &str) -> Option<String> {
    let mut cmd = Command::new("/usr/libexec/PlistBuddy");
    cmd.arg("-c").arg(format!("Print :{key}")).arg(plist);
    run(cmd, Duration::from_secs(5), false)
        .ok()
        .map(|o| String::from_utf8_lossy(&o).trim().to_string())
}

pub fn read_profile() -> Result<Profile> {
    let path = profile_path();
    let bytes = std::fs::read(&path).map_err(|_| {
        Error::actionable("iPhone signing isn't set up on this computer (no signing/wda.mobileprovision in berth-mobile's state folder)")
    })?;
    let tmp =
        std::env::temp_dir().join(format!("berth-mobile-profile-{}.plist", std::process::id()));
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["cms", "-D", "-i"]).arg(&path).arg("-o").arg(&tmp);
    run(cmd, Duration::from_secs(10), false)?;
    let app_id = plist_value(&tmp, "Entitlements:application-identifier").unwrap_or_default();
    let team = plist_value(&tmp, "TeamIdentifier:0").unwrap_or_default();
    let expires = plist_value(&tmp, "ExpirationDate").unwrap_or_default();
    let devices = plist_value(&tmp, "ProvisionedDevices")
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| l.len() > 20 && !l.contains('{') && !l.contains('}'))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    let prefix = app_id
        .strip_prefix(&format!("{team}."))
        .and_then(|p| p.strip_suffix(".*"))
        .ok_or_else(|| {
            Error::actionable(format!(
                "the iPhone signing profile needs a wildcard App ID (TEAM.prefix.*), not {app_id}"
            ))
        })?
        .to_string();
    Ok(Profile {
        team,
        prefix,
        devices,
        expires,
        sha: sha256_hex(&bytes),
    })
}

fn identity() -> Result<String> {
    let pass = std::fs::read_to_string(dir().join("keychain.pass"))
        .map_err(|_| Error::actionable("iPhone signing keychain is missing"))?;
    let kc = keychain();
    let mut unlock = Command::new("/usr/bin/security");
    unlock.args(["unlock-keychain", "-p", pass.trim()]).arg(&kc);
    run(unlock, Duration::from_secs(10), false)?;
    let mut find = Command::new("/usr/bin/security");
    find.args(["find-identity", "-v", "-p", "codesigning"])
        .arg(&kc);
    let out = String::from_utf8_lossy(&run(find, Duration::from_secs(10), false)?).into_owned();
    out.lines()
        .find(|l| l.contains("Apple Development"))
        .and_then(|l| l.split_whitespace().nth(1))
        .map(String::from)
        .ok_or_else(|| {
            Error::actionable(
                "no valid Apple Development identity in berth-mobile's signing keychain (is it in the keychain search list?)",
            )
        })
}

/// Checks the iPhone can run WebDriverAgent signed with this profile.
pub fn check_device(p: &Profile, udid: &str) -> Result<()> {
    if !p.devices.iter().any(|d| d.eq_ignore_ascii_case(udid)) {
        return Err(Error::actionable(format!(
            "iPhone {udid} isn't in berth-mobile's provisioning profile: add it to the profile in Apple Developer (Certificates, IDs & Profiles) and put the new profile in signing/wda.mobileprovision"
        )));
    }
    Ok(())
}

/// The signed runner app, signing it once per (WDA, profile, identity).
pub fn signed_wda(p: &Profile) -> Result<PathBuf> {
    let id = identity()?;
    let key = &sha256_hex(format!("{}{}{}", WDA.sha256, p.sha, id).as_bytes())[..16];
    let out_dir = crate::paths::cache_dir().join("wda").join(key);
    let app = out_dir.join("WebDriverAgentRunner-Runner.app");
    if app.join("embedded.mobileprovision").is_file() {
        return Ok(app);
    }
    let zip = crate::fetch::cached(&WDA, &crate::paths::cache_dir().join("tools"))?;
    let _ = std::fs::remove_dir_all(&out_dir);
    crate::paths::ensure_private(&out_dir)?;
    let mut unzip = Command::new("/usr/bin/ditto");
    unzip.arg("-x").arg("-k").arg(&zip).arg(&out_dir);
    run(unzip, Duration::from_secs(60), false)?;

    let xctest = app.join("PlugIns/WebDriverAgentRunner.xctest");
    for e in std::fs::read_dir(app.join("PlugIns"))?.flatten() {
        if e.file_name().to_string_lossy().ends_with(".dSYM") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let set_id = |plist: PathBuf, id: &str| -> Result<()> {
        let mut c = Command::new("/usr/libexec/PlistBuddy");
        c.arg("-c")
            .arg(format!("Set :CFBundleIdentifier {id}"))
            .arg(plist);
        run(c, Duration::from_secs(5), false).map(|_| ())
    };
    let runner = p.runner_bundle();
    set_id(app.join("Info.plist"), &runner)?;
    set_id(
        xctest.join("Info.plist"),
        runner.trim_end_matches(".xctrunner"),
    )?;
    std::fs::copy(profile_path(), app.join("embedded.mobileprovision"))?;
    let ent = out_dir.join("entitlements.plist");
    std::fs::write(
        &ent,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>application-identifier</key><string>{t}.{runner}</string>\n<key>com.apple.developer.team-identifier</key><string>{t}</string>\n<key>get-task-allow</key><true/>\n<key>keychain-access-groups</key><array><string>{t}.*</string></array>\n</dict></plist>\n",
            t = p.team
        ),
    )?;
    let sign = |path: &Path, entitlements: Option<&Path>| -> Result<()> {
        let mut c = Command::new("/usr/bin/codesign");
        c.args(["--force", "--sign", &id, "--keychain"])
            .arg(keychain());
        if let Some(e) = entitlements {
            c.arg("--entitlements").arg(e);
        }
        c.arg(path);
        run(c, Duration::from_secs(60), false).map(|_| ())
    };
    // inside out: frameworks, the test bundle, then the app
    if let Ok(entries) = std::fs::read_dir(xctest.join("Frameworks")) {
        for e in entries.flatten() {
            sign(&e.path(), None)?;
        }
    }
    sign(&xctest, None)?;
    sign(&app, Some(&ent))?;
    log(&format!("signed WebDriverAgent as {runner}"));
    Ok(app)
}

/// For `status`: None when signing is ready, else what is missing.
pub fn problem() -> Option<String> {
    match read_profile() {
        Err(e) => Some(e.message),
        Ok(_) => identity().err().map(|e| e.message),
    }
}
