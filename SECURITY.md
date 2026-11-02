# Security Policy

## Reporting a vulnerability

Report privately through GitHub: **Security > Report a vulnerability** on
[tinhthanh/mobile-mcp](https://github.com/tinhthanh/mobile-mcp/security). Please do not open a public issue.
Issues that also affect upstream should go to [mobile-next/mobile-mcp](https://github.com/mobile-next/mobile-mcp/security) as well.

## What this server can do to a device

mobile-lite gives an agent the same control over the phone as a person holding it. Use a phone set aside for testing.

- **Agent actions.** An agent can tap, type and launch anything. Prompt injection from screen content can steer it.
  The `android-tester` subagent refuses to send, call, pay or change settings unless the task asks for that exact
  action, but review what agents do. `open_url` and app install/uninstall from upstream were removed.
- **UiAutomator2 server.** While it runs (started on demand, stopped after 5 idle minutes and on exit) any app on the
  device can reach it on `localhost:6790` and drive the UI.
- **Network capture (`net`).**
  - The proxy mode sets the device's global `http_proxy`; it is cleared on stop, on exit and on the next run after a
    hard kill. Traffic goes over USB through `adb reverse`; nothing listens on the LAN.
  - Decrypting HTTPS needs the mitmproxy CA as a user CA. Whoever holds `~/.mitmproxy/mitmproxy-ca.pem` can then read
    the HTTPS of every app that trusts user CAs on that phone, browsers included. Install it only on test phones and
    remove it when done.
  - `show` redacts auth/cookie headers and credential-like values, but captured bodies can still hold personal data
    and end up in the agent's context.
  - PCAPdroid api keys live in `~/.config/mobile-mcp-lite/keys.env` (mode 600). A key lets any process with adb
    access start a capture without the on-device prompt.
- **HTTP transport.** `--listen` serves MCP over HTTP without auth unless `MOBILEMCP_AUTH` is set; prefer stdio.

## Supply chain

`scripts/setup.sh` pins every APK by SHA-256 (UiAutomator2 10.6.6: GitHub release identical to the npm package, which
has SLSA provenance; PCAPdroid 2.0.2: F-Droid build whose dex matches the developer's GitHub release). There is no
telemetry and no closed-source binary; `npm ci --ignore-scripts` is used everywhere.
