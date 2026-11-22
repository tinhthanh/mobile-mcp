---
name: berth-mobile
description: Use an Android phone or an iPhone plugged into the box over USB through the berth-mobile MCP server (Shipyard's "mobile" plugin) — open apps, read the screen as compact text, tap, type (Vietnamese ok), swipe, wait for text, run several steps at once, and capture which hosts or requests an app makes. Use for manual-QA checks, smoke tests, reproducing a UI bug on a real phone, or inspecting an app's network traffic.
---

# Android phones and iPhones with berth-mobile

The `berth-mobile` MCP server drives a real Android phone (adb) or iPhone
(WebDriverAgent) over USB. The phone belongs to a person: treat what is on it
as theirs.

## Loop

1. Omit `device` when one phone is connected. Call `devices` only when a tool
   says several are connected, or none is.
2. `screen` reads the screen as one line per element:
   `<mark><label>|x,y [flags]`. Marks: `*` tappable, `#` text input,
   `~` scrollable, space = plain text. Flags: `on`, `sel`, `off` (disabled),
   `focus`. x,y is the point to tap.
3. `tap`, `type`, `swipe`, `key`, `app` and `steps` return the new screen, or
   `unchanged`. **Never call `screen` right after one of them.**
4. When you know the next actions, send them in one `steps` call:
   `["launch com.x", "tap Login", "type a@b.c", "enter", "wait Home"]`.
   Prefer `tap <text>` over coordinates.
5. Use `wait {text}` (or `gone`) instead of reading the screen again and again.

One call at a time: never send several phone tools in parallel, the phone
needs them in order. Other agents may share the phone; a call waits up to two
minutes for them and then says the phone is busy.

## Reading what isn't in the tree

- Games, canvas or Flutter screens with few lines: `screen {ocr:true}`.
- Layout or colour checks: `screen {image:true}` — a JPEG, costs far more
  tokens than text; use only when the check is visual.
- `swipe up` reveals items below the fold; `tap <text>` fails fast with the
  current screen when the item is not visible.

## iPhone

- `devices` lists iPhones by UDID with `ios<version>`; a line ending in
  "(not usable: …)" says what the person must set up first.
- The first call starts WebDriverAgent on the phone (10–60 s); later reads
  take well under a second. It stops by itself when nobody uses the phone.
- `app` takes bundle ids: `com.apple.Preferences`, `com.apple.mobilesafari`;
  `app {action:"list"}` lists them.
- There is no BACK key: tap the app's back button (usually top left), or
  swipe right from the left edge. HOME works.
- `app=… (locked…)` or "The iPhone is locked": stop and ask the person to
  unlock it. Never try to enter the passcode.
- `net` is Android only.

## Network

`net` works only when the person turned on "net" for the mobile plugin in
Shipyard (Settings → Plugins); otherwise it says so — tell the person, don't
look for another way.

`net start`, exercise the app, `net list` (new since the last list),
`net show {id}` for one request's headers and body, and **always
`net stop`** before you finish: a capture left running keeps the phone on a
proxy. Apps that ignore the system proxy: `net start {app: "<package>"}`
(PCAPdroid, hosts only). HTTPS bodies only with `decrypt` and an app that
trusts the user CA.

## Safety

- Don't send messages, post, call, pay, buy, delete data, or change account
  or security settings unless the task asks for that exact action. If a step
  would do one of these and the task didn't ask, stop and report.
- Before an action others will see (sending a message), confirm on screen
  that the target (chat, recipient) is exactly the one asked for.
- Never type passwords or secrets you weren't given in the task, and never
  unlock the phone with a PIN on your own: ask the person.

## Report

Say what you saw, quoting screen lines as evidence: PASS / FAIL / BLOCKED
and why, each check with its evidence, anything unexpected (errors, slow
screens, crashes), and for network checks the hosts or requests seen.
