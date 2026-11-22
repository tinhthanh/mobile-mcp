# mobile-mcp (berth-mobile)

Token-lean MCP server that lets coding agents drive a real **Android phone** (adb) or **iPhone**
(WebDriverAgent) plugged in over USB, packaged as Shipyard's **"mobile"** plugin. One ~4.5 MB
universal macOS binary written in Rust, in [`berth-mobile/`](berth-mobile/). Fork of
[mobile-next/mobile-mcp](https://github.com/mobile-next/mobile-mcp) (Apache-2.0); the original Node
server was retired in favour of the Rust rewrite.

## Install

**With Shipyard:** Settings → Plugins → **Install from URL** →
`https://github.com/tinhthanh/vetgo-releases/releases/download/mobile-v<version>/mobile-<version>.tar.gz`
(leave the SHA-256 empty). Allow it; setup runs at once. Agents get the `berth-mobile` MCP server and skill.

**Without Shipyard (macOS):**

```bash
git clone https://github.com/tinhthanh/mobile-mcp.git && mobile-mcp/scripts/setup.sh --yes
```

Builds berth-mobile, fetches adb if needed, installs UiAutomator2 on connected phones, registers the
`berth-mobile` MCP server and the `android-tester` subagent in Claude Code. iPhones need signing set up once:
see [berth-mobile/README.md](berth-mobile/README.md#iphone). Vietnamese guide: [docs/SETUP.md](docs/SETUP.md).

Then: *"Use android-tester to check that Settings > Display shows Dark mode"*.

## Tools

`devices`, `screen`, `tap`, `type`, `swipe`, `key`, `app`, `wait`, `steps`, `net` (Android only). The screen is
one line per element with its tap point:

```
app=com.android.settings
*Wi‑Fi · Example WiFi|360,375
*Bluetooth · Off|360,495
#Search settings|371,267
```

Marks: `*` tappable, `#` text input, `~` scrollable, space = plain text; flags `on`, `sel`, `off`, `focus`.
Actions return the settled screen (or `unchanged`); `steps` runs a known sequence in one call; `wait` polls for
text; OCR (Apple Vision) fills in screens without an accessibility tree.

## What changed vs upstream

| | upstream mobile-mcp | this fork |
|---|---|---|
| Tools / schema tokens per session | 34 / ~7,500 | 10 / ~1,240 |
| Settings screen read (Android) | ~2,300 tokens, 1.2 s | ~95 tokens, 0.1–0.4 s (UiAutomator2 10.6.6) |
| iPhone screen read | mobilecli + DeviceKit | WebDriverAgent XML without `visible`: 0.3–0.7 s |
| Unicode / Vietnamese typing | DeviceKit app | UiAutomator2 / WebDriverAgent setText |
| Network inspection | — | `net`: mitmproxy over USB, PCAPdroid per app |
| Several agents on one phone | — | per-phone lock, shared server, idle stop |
| Runtime | Node + `mobilecli` (FSL-1.1) | one Rust binary, every download pinned by SHA-256 |
| Telemetry | PostHog + Scarf | none |

More: [berth-mobile/README.md](berth-mobile/README.md) (what runs, settings, iPhone, commands),
[SECURITY.md](SECURITY.md), [CHANGELOG.md](CHANGELOG.md).

## Other scripts

- `scripts/run-test.sh "<task>"` runs one android-tester task headless with the leanest context.
- `scripts/setup.sh mirror` shows and controls the phone on the Mac over USB with scrcpy.
- `berth-mobile/scripts/package.sh` builds the plugin; `berth-mobile/scripts/signing-backup.sh` backs up the iOS
  signing keys.

## License

Apache-2.0, see [LICENSE](LICENSE).
