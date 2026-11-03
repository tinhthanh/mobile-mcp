# mobile-mcp-lite

Token-lean MCP server for driving a real Android phone over `adb`, plus an `android-tester` subagent for
Claude Code. Fork of [mobile-next/mobile-mcp](https://github.com/mobile-next/mobile-mcp) (Apache-2.0),
trimmed to Android only. No root needed.

## Quick start (macOS)

```bash
git clone https://github.com/tinhthanh/mobile-mcp.git && mobile-mcp/scripts/setup.sh
```

One command, safe to re-run (~6 s when nothing changed): installs adb and Node if missing, builds, waits for a phone
(telling you to enable USB debugging or tap *Allow* if needed), installs the pinned UiAutomator2 server on every
connected phone and checks it reads the screen, then registers the `mobile-lite` MCP server, the `android-tester`
subagent and (after asking) the `mcp__mobile-lite` permission in Claude Code. Add `--net` for network capture
(mitmproxy, PCAPdroid, its api key and VPN consent, all automatic), `--with-ca` to push the mitmproxy CA,
`--serial <id>` for one phone, `--yes` to skip the question. `scripts/setup.sh verify` checks a phone.
`scripts/setup.sh mirror install` shows the phone's screen on the Mac with mouse and keyboard control (scrcpy over
USB) whenever it is plugged in: no consent dialog, no accessibility service, nothing reachable over Wi‑Fi.

Then, in a new Claude Code session: *"Use android-tester to check that Settings > Display shows Dark mode"*.
Manual steps, troubleshooting and cleanup: [docs/SETUP.md](docs/SETUP.md) (Vietnamese).

## What changed vs upstream

Measured on a ZTE Blade A54 (Android 13).

| | upstream mobile-mcp | lite |
|---|---|---|
| Tools / schema tokens per session | 34 / ~7,500 | 10 / ~1,240 |
| Settings screen read | ~2,300 tokens, 1.2 s | ~95 tokens, ~0.08 s (UiAutomator2 server 10.6.6) |
| Action then observe | 2 calls | 1 call (actions return the new screen, or `unchanged`) |
| Known sequences | `mobile_batch_commands` | `steps` with short actions, `wait` for text |
| Accessibility tree empty (games, canvas) | screenshot (~600+ image tokens) | Apple Vision OCR on macOS (~80 tokens) |
| Unicode / Vietnamese typing | needs DeviceKit app | UiAutomator2 `setText` |
| Network inspection | — | `net`: mitmproxy over USB, PCAPdroid per app |
| Native binary | `mobilecli` (FSL-1.1, closed build) | none, plain `adb` |
| Telemetry | PostHog + Scarf | none |
| iOS, cloud devices, recording, install helper | yes | removed |

A 3-part smoke test (UI check, Vietnamese typing, Zalo network) costs 7 turns, 38 s, $0.037 with
`scripts/run-test.sh` on Sonnet.

## Screen format

```
app=com.android.settings
*Wi‑Fi · Example WiFi|360,375
*Bluetooth · Off|360,495
#Search settings|371,267
~recycler_view|360,918
```

`<mark><label>|x,y [flags]`: x,y is the point to tap. Marks: `*` tappable, `#` text input,
`~` scrollable, space = plain text. Flags: `on`, `sel`, `off` (disabled), `focus`.
Texts inside a tappable row are merged into it; id-only views stacked on a labelled one are dropped.

## Tools

`devices`, `screen {ocr?, image?}`, `tap {x,y | text, mode?}`, `type {text, submit?}`, `swipe {direction, x?, y?, distance?}`,
`key {button}`, `app {launch|stop|list, package?}`, `wait {text, gone?, timeout?}`, `steps {steps[]}`, `net {...}`.

- `device` is optional when one device is connected.
- Actions accept `observe=false` to skip reading the screen afterwards. With the UiAutomator2 server they return the
  screen once two reads agree, so coordinates are not taken mid-animation.
- `tap {text}` finds the element on the current screen (exact label on a tappable line first; the first of several
  matches is tapped and reported).
- Calls run one at a time, even when a client sends several in parallel.

`steps` runs a known sequence in one call and returns the final screen, stopping at the first failure with the
current screen:

```json
{"steps": ["launch com.android.settings", "tap Display", "wait Dark mode", "key BACK"]}
```

Actions: `tap <text|x,y>`, `long <text|x,y>`, `type <text>`, `enter`, `key <BUTTON>`, `swipe <dir>`, `wait <text>`,
`gone <text>`, `launch <package>`, `stop <package>`, `sleep <ms>`.

## Network capture: `net`

`net {start | list | show {id} | stop}` captures the device's HTTP(S) requests with mitmproxy, reached over USB
with `adb reverse` plus the device's global `http_proxy`: nothing listens on the LAN.

```
#1 POST 200 httpbin.org/post 481B 557ms application/json
tls (not decrypted): www.google.com, api.example.com×3
```

- `list` returns only requests since the previous list (`all=true` for everything), `filter` matches host/path.
- HTTPS is passed through and only the host is recorded, so apps keep working. `start {decrypt:"api\\.myapp\\.com"}`
  decrypts matching hosts; that needs the mitmproxy CA installed as a user CA and an app that trusts user CAs
  (a debug build with `network_security_config`, see [docs/SETUP.md](docs/SETUP.md)).
- Only apps honoring the system proxy show up (OkHttp/HttpURLConnection do; Zalo, Flutter and custom sockets mostly do not).
  `start {app:"com.zing.zalo"}` captures one app with [PCAPdroid](https://f-droid.org/packages/com.emanuelef.remote_capture/)
  instead (per-app VPN): every connection, hosts only, one line per destination from TLS SNI, HTTP Host and DNS
  (`tls api.example.com:443 conns=2 bytes=6310`).
- `show` hides browser/transport headers unless `all=true`, and redacts auth/cookie headers, credential-like keys and
  bearer tokens in bodies; bodies are cut at 2,000 chars.
- The proxy is cleared on `stop` and on exit. A proxy left by a server killed hard is cleared the next time the server
  touches the device; to fix it at once: `adb shell settings put global http_proxy :0`.

## android-tester subagent

[`.claude/agents/android-tester.md`](.claude/agents/android-tester.md): Sonnet, only the mobile-lite tools, cost rules
(`steps`, `wait`, images only for visual checks), safety rules (no sending, calling, paying or settings changes unless
the task asks for exactly that) and a PASS/FAIL report quoting screen lines.

- From Claude Code: delegate to it by name. Add `"permissions": {"allow": ["mcp__mobile-lite"]}` to
  `~/.claude/settings.json` so its calls are not prompted one by one.
- Headless, cheapest: `scripts/run-test.sh "<task>"` (prints the report and `[turns, seconds, $]`, exits 1 unless PASS;
  `MODEL=haiku`, `STREAM=run.jsonl`).

## Configuration

| Variable | Effect |
|---|---|
| `MOBILEMCP_KEYS_FILE` | PCAPdroid api keys (`PCAPDROID_API_KEY_<serial>=…`), default `~/.config/mobile-mcp-lite/keys.env` |
| `PCAPDROID_API_KEY[_<serial>]` | Same keys from the environment, read before the file |
| `MOBILEMCP_DISABLE_UIA2=1` | Do not use the UiAutomator2 server (slower reads, ASCII-only typing) |
| `MOBILEMCP_ALL_VIEWS=1` | Keep unimportant views in page source (slower) |
| `MOBILEMCP_DISABLE_OCR=1` / `MOBILEMCP_OCR_BIN` | Turn OCR off / use a prebuilt OCR binary |
| `MOBILEMCP_MITMDUMP` | Path to `mitmdump` |
| `LOG_FILE` | Append the server log to this file |

Without `scripts/setup.sh`: `npm ci --ignore-scripts && npm run build && claude mcp add -s user mobile-lite -- node "$PWD/lib/index.js"`.
Needs `adb` on PATH (or `ANDROID_HOME`); OCR needs Xcode command line tools (`native/ocr.swift` is compiled once into
`~/Library/Caches/mobile-mcp-lite/`).

## UiAutomator2 server

With `io.appium.uiautomator2.server` (+ `.test`) installed, the server keeps UiAutomation alive on the device: screen
reads take ~0.2 s instead of ~2.7 s for `uiautomator dump`, and `type` accepts any Unicode text. `scripts/setup.sh`
installs 10.6.6 with pinned hashes.

- Started on first use, stopped after 5 idle minutes and on exit. While it runs, any app on the device can reach it
  on `localhost:6790`.
- If page source keeps failing on a screen, reads fall back to `uiautomator dump` for 60 s.

## Known limits

- `type` replaces the field with the UiAutomator2 server, but appends (ASCII only) without it.
- `net list` in PCAPdroid mode returns the whole summary each time, not only new hosts.
- OCR can still read some icons as short words.
- The api key step of `scripts/setup.sh --net` relies on PCAPdroid's English UI; other languages need the manual step.

## License

Apache-2.0, as upstream. This fork rewrites `src/server.ts` and adds `src/screen.ts`, `src/net.ts`, `src/uia2.ts`,
`src/ocr.ts`, `native/` and `scripts/`; see [CHANGELOG.md](CHANGELOG.md) for the changes.
