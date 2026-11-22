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
    /// When the profile or the certificate in it runs out, whichever is first.
    pub expires_unix: Option<i64>,
    pub sha: String,
}

impl Profile {
    /// Whole days until signing runs out (negative once it has).
    pub fn days_left(&self) -> Option<i64> {
        self.expires_unix
            .map(|t| (t - crate::util::unix_secs() as i64).div_euclid(86_400))
    }

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
    let mut ex = Command::new("/usr/bin/plutil");
    ex.args(["-extract", "ExpirationDate", "raw", "-o", "-"])
        .arg(&tmp);
    let profile_end = run(ex, Duration::from_secs(5), false)
        .ok()
        .and_then(|o| parse_iso(String::from_utf8_lossy(&o).trim()));
    let cert_end = cert_not_after();
    let expires_unix = match (profile_end, cert_end) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
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
        expires_unix,
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

/// Seconds since 1970 for "2027-11-22T11:19:26Z".
fn parse_iso(s: &str) -> Option<i64> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(|x| x.parse::<i64>());
    let (h, mi, sec) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + mi * 60 + sec)
}

/// The signing certificate's notAfter, from signing/dev.cer.
fn cert_not_after() -> Option<i64> {
    let mut c = Command::new("/usr/bin/openssl");
    c.args(["x509", "-inform", "DER", "-noout", "-enddate", "-in"])
        .arg(dir().join("dev.cer"));
    let out = String::from_utf8_lossy(&run(c, Duration::from_secs(5), false).ok()?).into_owned();
    // notAfter=Nov 22 11:06:50 2027 GMT
    let v: Vec<&str> = out
        .trim()
        .strip_prefix("notAfter=")?
        .split_whitespace()
        .collect();
    let first = *v.first()?;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == first)?
        + 1;
    parse_iso(&format!(
        "{}-{:02}-{:02}T{}Z",
        v.get(3)?,
        month,
        v.get(1)?.parse::<u32>().ok()?,
        v.get(2)?
    ))
}

/// Checks the iPhone can run WebDriverAgent signed with this profile.
pub fn check_device(p: &Profile, udid: &str) -> Result<()> {
    if p.days_left().is_some_and(|d| d < 0) {
        return Err(Error::actionable(format!(
            "berth-mobile's iPhone signing expired ({}): make a new certificate and profile in Apple Developer and put them in signing/",
            p.expires
        )));
    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn parses_iso_dates() {
        assert_eq!(super::parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            super::parse_iso("2027-11-22T11:19:26Z"),
            Some(1_826_882_366)
        );
        assert_eq!(super::parse_iso("2000-03-01T00:00:00Z"), Some(951_868_800));
    }
}
