# Security Policy

## Reporting a vulnerability

Report privately through GitHub: **Security > Report a vulnerability** on
[tinhthanh/mobile-mcp](https://github.com/tinhthanh/mobile-mcp/security). Please do not open a public issue.
Issues that also affect upstream should go to [mobile-next/mobile-mcp](https://github.com/mobile-next/mobile-mcp/security) as well.

## What berth-mobile can do to a device

It gives an agent the same control over the phone as a person holding it. Use a phone set aside for testing.

- **Agent actions.** An agent can tap, type and launch anything. Prompt injection from screen content can steer it.
  The skill and the `android-tester` subagent tell agents not to send, call, pay or change settings unless the task
  asks for that exact action, and never to enter a passcode; review what agents do.
- **UiAutomator2 (Android).** While it runs (started on demand, stopped when no process used it for 5 minutes, and
  on exit) any app on the phone can reach it on `localhost:6790` and drive the UI.
- **WebDriverAgent (iPhone).** Runs with `USE_IP=127.0.0.1`: it listens on the phone's loopback only and is reached
  over the cable, never on Wi‑Fi. Any app on the iPhone could still reach it on `localhost:8100` while it runs; it
  stops when no process used it for 5 minutes.
- **Network capture (`net`, Android, off unless the person turns it on in Shipyard).**
  - The proxy mode sets the phone's global `http_proxy`; it is cleared on stop, when mitmdump exits and on the next
    run after a hard kill. Traffic goes over USB through `adb reverse`; nothing listens on the LAN.
  - Decrypting HTTPS needs the mitmproxy CA as a user CA. Whoever holds `~/.mitmproxy/mitmproxy-ca.pem` can then
    read the HTTPS of every app that trusts user CAs on that phone. Install it only on test phones.
  - `show` redacts auth/cookie headers and credential-like values, but bodies can still hold personal data.
  - PCAPdroid api keys live in `~/Library/Application Support/berth-mobile/keys.env` or
    `~/.config/mobile-mcp-lite/keys.env` (mode 600).

## Signing keys (iPhone)

The App Store Connect API key and the Apple Development identity live in
`~/Library/Application Support/berth-mobile/{asc,signing}` (mode 600, the identity in a keychain of its own).
`berth-mobile/secrets/berth-mobile-signing.tar.enc` is an encrypted backup of them (AES-256-CBC, PBKDF2-SHA256
600k rounds); this repository is public, so its safety is the passphrase's strength. If it may have leaked, revoke
the API key in App Store Connect and the certificate in Apple Developer.

## Supply chain

Everything berth-mobile downloads is pinned by SHA-256 in the binary, which Shipyard pins in turn: adb
(platform-tools r37.0.1), UiAutomator2 10.6.6 (GitHub release, identical to the npm package with SLSA provenance),
PCAPdroid 2.0.2 (F-Droid build whose dex matches the developer's release), go-ios 1.3.2, WebDriverAgent 16.15.3 and
the iOS developer disk image (mounted only with Apple's TSS signature). No telemetry, no closed-source binary.
