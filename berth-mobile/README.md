# berth-mobile — Shipyard's "mobile" plugin (Android and iPhone)

The mobile-mcp-lite tools (`devices`, `screen`, `tap`, `type`, `swipe`, `key`,
`app`, `wait`, `steps`, `net`) rewritten in Rust as one ~4.5 MB universal
macOS binary, packaged as a Shipyard plugin. Agents on a box get an MCP server
`berth-mobile` and a skill `berth-mobile`; the screen format and tool behaviour
are the same as the Node server's.

## Install

Shipyard → Settings → Plugins → **Install from URL**:
`https://github.com/tinhthanh/vetgo-releases/releases/download/mobile-v<version>/mobile-<version>.tar.gz`
(leave the SHA-256 empty: the `.sha256` beside it is used). Allow it; its setup
runs at once. To put it on a box, turn the box on under the plugin.

Local build: `scripts/package.sh --install` (needs Xcode command line tools
and the `x86_64-apple-darwin` Rust target), then allow it in Settings → Plugins.

## What runs, and what was allowed

Shipyard pins `bin/berth-mobile` and the skill by SHA-256. Nothing else is
run from the plugin folder:

| Needed | Where it comes from |
|---|---|
| OCR helper (Apple Vision) | built from `../native/ocr.swift` into the binary, unpacked to the cache and re-checked |
| mitmproxy addon | `../native/net_addon.py`, built into the binary, written to a private temp folder per capture |
| adb | an adb 37.0.1 already on the computer, else Google's platform-tools r37.0.1 (SHA-256 pinned in the binary) — the same build vglink uses, so two adb servers don't fight |
| UiAutomator2 10.6.6 | Appium's GitHub release APKs, pinned; installed on a phone the first time it's used (setting `auto-uia2`, on by default) |
| PCAPdroid 2.0.2 | F-Droid APK, pinned; installed for `net start {app}` only (GPL, never shipped) |
| mitmdump | the computer's own (`brew install --cask mitmproxy`), for `net start` |

State lives outside the plugin folder (Shipyard copies that folder to boxes
and replaces it on updates): `~/Library/Application Support/berth-mobile`
(fetched adb, `keys.env` for PCAPdroid api keys, locks) and
`~/Library/Caches/berth-mobile` (APKs, OCR helper). mobile-mcp-lite's
`~/.config/mobile-mcp-lite/keys.env` is still read.

## Settings (Settings → Plugins → Mobile)

- `auto-uia2` (on): install UiAutomator2 on a phone that lacks it.
- `net` (off, asks first): lets agents use `net` — set the phone's HTTP proxy
  to mitmproxy on this computer, or install PCAPdroid and capture one app.

## Several agents, one phone

Every agent runs its own berth-mobile process. A tool call holds a per-phone
file lock (`run/<serial>/lock`, waits up to 2 min), so actions from two agents
never interleave. The UiAutomator2 server is shared: a process reuses the
session that is there, and stops the server (idle 5 min, or on exit) only when
no other live process used it in the last 5 minutes (`run/<serial>/uia2.<pid>`).

## Commands

```
berth-mobile [mcp]        MCP server on stdio
berth-mobile setup        fetch adb if needed, install UiAutomator2 on connected phones (log: state/setup.log)
berth-mobile status       JSON for Shipyard: {ready, problem, adb, adbVersion, phones, settings}
berth-mobile settings     JSON list of the switches
```

Environment: `BERTH_MOBILE_ADB` (adb to use), `BERTH_MOBILE_HOME`,
`BERTH_MOBILE_CACHE`, `LOG_FILE`, `MOBILEMCP_DISABLE_UIA2`,
`MOBILEMCP_DISABLE_OCR`, `MOBILEMCP_ALL_VIEWS`, `MOBILEMCP_MITMDUMP`,
`PCAPDROID_API_KEY[_<serial>]`.

## iPhone

iOS runs nothing that isn't signed for the device, so an iPhone needs, once
per Mac, an Apple Developer team's signing material in
`~/Library/Application Support/berth-mobile/signing/`:

- `berth-mobile.keychain-db` + `keychain.pass` — an Apple Development identity
  whose key was made on this Mac, in a keychain of its own (added to the end
  of the user's keychain search list: `codesign` only looks there);
- `wda.mobileprovision` — a development profile for a wildcard App ID
  (`TEAM.prefix.*`) listing the iPhones.

The iPhone must trust the Mac and have Developer Mode on. Then, on first use,
berth-mobile fetches go-ios 1.3.2 (MIT, pinned), Appium's WebDriverAgent
16.15.3 (BSD, pinned, re-signed as `prefix.WebDriverAgentRunner.xctrunner`) and
the iOS 17+ developer disk image (pinned; the phone mounts it only with
Apple's TSS signature), installs WebDriverAgent, starts go-ios's userspace
tunnel (no root) and runs WebDriverAgent with `USE_IP=127.0.0.1`: it listens
on the phone's loopback only, reached through `ios forward` over the cable,
never on Wi-Fi. It stops when no process used the phone for 5 minutes.

An App Store Connect API key with the Developer role can read devices and
create certificates but not App IDs or profiles: make those in the Apple
Developer site (Certificates, IDs & Profiles).

## Not yet

Linux and Windows boxes: the binary is macOS-only for now. Windows also needs
Shipyard's `berthd plugin-mcp` to stop using `syscall.Exec`.
