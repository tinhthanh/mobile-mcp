#!/usr/bin/env bash
# Phone screen on the Mac over USB, with mouse and keyboard control (scrcpy). No consent
# dialog, no accessibility service: scrcpy runs as the adb shell user, so the only phone-side
# step is "Allow USB debugging" with "Always allow from this computer", once.
#
#   scripts/mirror.sh [serial]       open the phone's screen now (waits for a phone)
#   scripts/mirror.sh install        open it by itself whenever a phone is plugged in (launchd)
#   scripts/mirror.sh uninstall      stop doing that
#   scripts/mirror.sh status         is the launchd agent loaded, which phones are open
#
# While mirrored the phone stays awake; unplugging ends it. Closing the window keeps it closed
# until the phone is plugged in again. A locked phone is unlocked by typing its PIN in the window.
set -euo pipefail

LABEL="com.mobile-mcp-lite.mirror"
# launchd may not read ~/Documents (privacy protection), so the agent runs a copy from here
AGENT_SCRIPT="$HOME/Library/Application Support/mobile-mcp-lite/mirror.sh"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG_DIR="$HOME/Library/Logs/mobile-mcp-lite"
STATE="${TMPDIR:-/tmp}/mobile-mcp-lite-mirror"
SCRCPY_ARGS=(--stay-awake --no-audio)

ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }

need_tools() {
	command -v adb >/dev/null || fail "Chưa có adb — chạy scripts/setup.sh host"
	if ! command -v scrcpy >/dev/null; then
		command -v brew >/dev/null || fail "Chưa có scrcpy và Homebrew — cài scrcpy trước"
		brew install scrcpy
	fi
}

# serials of phones on USB that allowed debugging (`adb devices -l` marks USB ones with usb:)
usb_phones() { adb devices -l | awk '$2 == "device" && / usb:/ { print $1 }'; }

open_phone() {
	local serial="$1" model
	model=$(adb -s "$serial" shell getprop ro.product.model 2>/dev/null | tr -d '\r')
	scrcpy -s "$serial" --window-title "${model:-$serial}" "${SCRCPY_ARGS[@]}"
}

cmd_open() {
	need_tools
	local serial="${1:-}"
	if [ -z "$serial" ]; then
		printf '  … chờ điện thoại cắm USB (đã bật gỡ lỗi USB)\n'
		until serial=$(usb_phones | head -1) && [ -n "$serial" ]; do sleep 2; done
	fi
	open_phone "$serial"
}

# launchd runs this: one window per plugged-in phone. A window closed by hand stays closed
# until that phone is unplugged, so the loop never fights the user.
cmd_watch() {
	mkdir -p "$STATE"
	rm -f "$STATE"/*
	while true; do
		local plugged
		plugged=$(usb_phones || true)
		for f in "$STATE"/*; do
			[ -e "$f" ] || continue
			local serial
			serial=$(basename "$f")
			if ! grep -qx "$serial" <<<"$plugged"; then
				rm -f "$f"
			fi
		done
		for serial in $plugged; do
			[ -e "$STATE/$serial" ] && continue
			touch "$STATE/$serial"
			echo "$(date '+%F %T') open $serial"
			open_phone "$serial" &
		done
		sleep 2
	done
}

cmd_install() {
	need_tools
	mkdir -p "$(dirname "$PLIST")" "$LOG_DIR" "$(dirname "$AGENT_SCRIPT")"
	cp "$0" "$AGENT_SCRIPT"
	# launchd starts with a bare PATH; give it the ones adb and scrcpy live in
	local path
	path="$(dirname "$(command -v adb)"):$(dirname "$(command -v scrcpy)"):/usr/bin:/bin:/usr/sbin:/sbin"
	cat >"$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$LABEL</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/bash</string>
		<string>$AGENT_SCRIPT</string>
		<string>watch</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict><key>PATH</key><string>$path</string></dict>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key><true/>
	<key>LimitLoadToSessionType</key><string>Aqua</string>
	<key>StandardOutPath</key><string>$LOG_DIR/mirror.log</string>
	<key>StandardErrorPath</key><string>$LOG_DIR/mirror.log</string>
</dict>
</plist>
PLIST
	launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
	launchctl bootstrap "gui/$(id -u)" "$PLIST"
	ok "Đã bật: cắm điện thoại là màn hình tự mở trên Mac (log: $LOG_DIR/mirror.log)"
}

cmd_uninstall() {
	launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
	rm -f "$PLIST" "$AGENT_SCRIPT"
	ok "Đã tắt tự mở màn hình khi cắm cáp"
}

cmd_status() {
	if launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1; then ok "Tự mở khi cắm cáp: đang bật"; else printf '  – Tự mở khi cắm cáp: tắt\n'; fi
	pgrep -fl '^scrcpy ' | sed 's/^/  /' || printf '  – Không có cửa sổ nào đang mở\n'
}

case "${1:-}" in
	install) cmd_install ;;
	uninstall) cmd_uninstall ;;
	status) cmd_status ;;
	watch) cmd_watch ;;
	-h|--help) sed -n '2,14p' "$0" ;;
	*) cmd_open "${1:-}" ;;
esac
