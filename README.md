# mobile-mcp-lite

Token-lean MCP server for driving a real Android device over `adb`. Fork of
[mobile-next/mobile-mcp](https://github.com/mobile-next/mobile-mcp) (Apache-2.0), trimmed to Android only.

## What changed vs upstream

| | upstream mobile-mcp | lite |
|---|---|---|
| Tools / schema tokens per session | 34 / ~7,500 | 8 / ~1,000 |
| Settings screen read | ~2,300 tokens | ~95 tokens |
| Action then observe | 2 calls | 1 call (actions return the new screen, or `unchanged`) |
| Accessibility tree empty (games, canvas) | screenshot (~600+ image tokens) | Apple Vision OCR on macOS (~80 tokens) |
| Screen read latency (ZTE Blade A54) | — | ~0.4 s with UiAutomator2 server, ~2.7 s with plain `uiautomator dump` |
| Unicode / Vietnamese typing | needs DeviceKit app | UiAutomator2 `setText` |
| Native binary | `mobilecli` (FSL-1.1, closed build) | none, plain `adb` |
| Telemetry | PostHog + Scarf | none |
| iOS, cloud devices, recording, install helper | yes | removed |

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
Texts inside a tappable row are merged into it.

## Tools

`devices`, `screen {ocr?}`, `tap {x,y | text, mode?}`, `type {text,submit?}`, `swipe {direction,x?,y?,distance?}`,
`key {button}`, `app {launch|stop|list, package?}`, `screenshot`.
Every action accepts `observe=false` to skip reading the screen afterwards. With the UiAutomator2 server,
actions return the screen once two reads agree, so coordinates are not taken mid-animation.
`tap {text}` finds the element on the current screen (exact label on a tappable line first) and taps it in one call.

## Network capture: `net`

`net {start | list | show {id} | stop}` captures the device's HTTP(S) requests with mitmproxy
(`brew install mitmproxy`), reached over USB with `adb reverse` plus the device's global `http_proxy`:
no root, nothing listening on the LAN.

```
#1 POST 200 httpbin.org/post 481B 557ms application/json
tls (not decrypted): www.google.com, api.example.com×3
```

- `list` returns only requests since the previous list (`all=true` for everything), `filter` matches host/path.
- HTTPS is passed through and only the host is recorded, so apps keep working. `start {decrypt:"api\\.myapp\\.com"}`
  decrypts matching hosts; that needs the mitmproxy CA installed as a user CA and an app that trusts user CAs
  (a debug build with `network_security_config`).
- Only apps honoring the system proxy show up (OkHttp/HttpURLConnection do; Zalo, Flutter and custom sockets mostly do not).
- `show` redacts auth/cookie headers and credential-like keys and bearer tokens in bodies; bodies are cut at 2,000 chars.
- The proxy is cleared on `stop` and when the server exits. If the server is killed hard, restore the device's
  internet with `adb shell settings put global http_proxy :0`.

## Install (Claude Code)

```bash
npm ci --ignore-scripts && npm run build
claude mcp add mobile-lite -- node "$PWD/lib/index.js"
```

Needs `adb` on PATH (or `ANDROID_HOME`). OCR needs macOS with Xcode command line tools; the Swift helper in
`native/ocr.swift` is compiled once into `~/Library/Caches/mobile-mcp-lite/`. Set `MOBILEMCP_DISABLE_OCR=1` to turn it off.

## Fast mode: Appium UiAutomator2 server (optional)

If `io.appium.uiautomator2.server` and `io.appium.uiautomator2.server.test` are installed, the server keeps
UiAutomation alive on the device: screen reads drop to ~0.4 s and `type` accepts any Unicode text.
Install the official APKs from [appium-uiautomator2-server releases](https://github.com/appium/appium-uiautomator2-server/releases)
and check their SHA-256 against the release before `adb install`.

- Started on first use, stopped after 5 idle minutes and on exit. While it runs, any app on the device
  can reach it on `localhost:6790`.
- If page source keeps failing on a screen, it falls back to `uiautomator dump` for 60 s.
- `MOBILEMCP_DISABLE_UIA2=1` turns it off.
- `hidden_api_policy` is left untouched, so a few screens (e.g. Android 13's clipboard overlay) cannot be read
  through it and use the fallback.

## Known limits

- `type` replaces the field when the UiAutomator2 server is used, but appends (ASCII only) without it.
