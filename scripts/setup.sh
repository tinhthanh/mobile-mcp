#!/usr/bin/env bash
# Set up berth-mobile on a Mac without Shipyard, or next to it. In Shipyard, install the
# "mobile" plugin instead (Settings → Plugins → Install from URL, see berth-mobile/README.md):
# it brings the same binary, runs its setup and gives agents the berth-mobile MCP server.
#
#   scripts/setup.sh            build berth-mobile, fetch adb if needed, install UiAutomator2
#                               on connected phones (and WebDriverAgent on iPhones when signing
#                               is set up), link the android-tester agent
#     --yes                     also allow mcp__berth-mobile in ~/.claude/settings.json
#   scripts/setup.sh mirror …   phone screen and control on the Mac over USB (scrcpy); see scripts/mirror.sh
#
# Pins (adb, UiAutomator2, PCAPdroid, go-ios, WebDriverAgent) live in the binary, checked by SHA-256.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
[ "${1:-}" = mirror ] && { shift; exec "$REPO/scripts/mirror.sh" "$@"; }
YES=""
[ "${1:-}" = --yes ] || [ "${1:-}" = -y ] && YES=1

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }

step "Build berth-mobile"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null || { echo "needs Rust (https://rustup.rs) and Xcode command line tools" >&2; exit 1; }
(cd "$REPO/berth-mobile" && cargo build --release --locked -q)
BIN="$REPO/berth-mobile/target/release/berth-mobile"
ok "$("$BIN" version)"

step "Phones"
if "$BIN" setup; then ok "setup done"; else warn "setup had problems: see ~/Library/Application Support/berth-mobile/setup.log"; fi
"$BIN" status | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
print("  adb:", d.get("adb"), d.get("adbVersion"))
for p in d["phones"]: print("  android", p["serial"], p["state"], "uia2" if p.get("uia2") else "")
for p in d["iphones"]: print("  iphone ", p["udid"], p["signing"])
for x in d["problem"]: print("  ! " + x)'

step "Claude Code"
mkdir -p "$HOME/.claude/agents"
ln -sfn "$REPO/.claude/agents/android-tester.md" "$HOME/.claude/agents/android-tester.md"
ok "subagent android-tester (~/.claude/agents)"
if [ -f "$HOME/.berth/plugins/mobile/berth-plugin.json" ]; then
	ok "MCP berth-mobile comes from Shipyard's mobile plugin"
elif command -v claude >/dev/null; then
	claude mcp remove berth-mobile -s user >/dev/null 2>&1 || true
	claude mcp add -s user berth-mobile -- "$BIN" mcp >/dev/null
	ok "registered MCP berth-mobile → $BIN"
fi
SETTINGS="$HOME/.claude/settings.json"
if [ -n "$YES" ]; then
	python3 -I - "$SETTINGS" <<'PY'
import json, os, sys
path = sys.argv[1]
data = json.load(open(path)) if os.path.exists(path) else {}
allow = data.setdefault("permissions", {}).setdefault("allow", [])
if "mcp__berth-mobile" not in allow:
    allow.append("mcp__berth-mobile")
    with open(path, "w") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")
PY
	ok "mcp__berth-mobile allowed in $SETTINGS"
else
	warn "Claude Code asks before each phone action; rerun with --yes to allow mcp__berth-mobile"
fi
