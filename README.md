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

`devices`, `screen {ocr?}`, `tap {x,y,mode?}`, `type {text,submit?}`, `swipe {direction,x?,y?,distance?}`,
`key {button}`, `app {launch|stop|list, package?}`, `screenshot`.
Every action accepts `observe=false` to skip reading the screen afterwards.

## Install (Claude Code)

```bash
npm ci --ignore-scripts && npm run build
claude mcp add mobile-lite -- node "$PWD/lib/index.js"
```

Needs `adb` on PATH (or `ANDROID_HOME`). OCR needs macOS with Xcode command line tools; the Swift helper in
`native/ocr.swift` is compiled once into `~/Library/Caches/mobile-mcp-lite/`. Set `MOBILEMCP_DISABLE_OCR=1` to turn it off.

## Known limits

- `uiautomator dump` takes ~2.7 s on low-end phones; a persistent on-device service would cut this.
- Non-ASCII text (e.g. Vietnamese) still needs the upstream DeviceKit app for clipboard paste.
