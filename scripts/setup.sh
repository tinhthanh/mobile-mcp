#!/usr/bin/env bash
# Set up mobile-mcp-lite on a Mac and on an Android device. See docs/SETUP.md.
#
#   scripts/setup.sh host                 adb, mitmproxy, build
#   scripts/setup.sh device [serial]      UiAutomator2 server, PCAPdroid (+ api key), mitmproxy CA file
#   scripts/setup.sh register             add the MCP server to Claude Code (once per Mac)
#   scripts/setup.sh verify [serial]      check everything on a device
#   scripts/setup.sh clean [serial]       remove what `device` installed (the user CA is removed by hand)
#
# Every APK is pinned to the SHA-256 that was checked against its official release
# (UiAutomator2: GitHub release == npm package apks, the npm package has SLSA provenance).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
UI="node $REPO/scripts/device-ui.cjs"
CACHE="$HOME/Library/Caches/mobile-mcp-lite/apk"
CONFIG="$HOME/.config/mobile-mcp-lite"
KEYS="$CONFIG/keys.env"

UIA2_VERSION="10.6.6"
UIA2_URL="https://github.com/appium/appium-uiautomator2-server/releases/download/v$UIA2_VERSION"
UIA2_SERVER_APK="appium-uiautomator2-server-v$UIA2_VERSION.apk"
UIA2_SERVER_SHA="8ff760a2a86b487f53090fbdcd5b0360e67d02bb811887d527a9557b0d59c80d"
UIA2_TEST_APK="appium-uiautomator2-server-debug-androidTest.apk"
UIA2_TEST_SHA="e6f729287d72351388fd66597c35b69988b9091a7011954bf92a55d416cb1e1c"

# F-Droid build of PCAPdroid 2.0.2 (its dex matches the developer's GitHub release)
PCAPDROID_PKG="com.emanuelef.remote_capture"
PCAPDROID_APK="com.emanuelef.remote_capture_95.apk"
PCAPDROID_URL="https://f-droid.org/repo"
PCAPDROID_SHA="e03f79082278b3754b4954c7537e7c9f7681e5995a44e70aa1589eca10190066"
PCAPDROID_CTRL="$PCAPDROID_PKG/.activities.CaptureCtrl"

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$*"; exit 1; }
manual() { printf '  \033[36m→ LÀM TAY:\033[0m %s\n' "$*"; }

pick_serial() {
	if [ -n "${1:-}" ]; then
		SERIAL="$1"
		return
	fi

	local devices
	devices=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }')
	if [ "$(printf '%s\n' "$devices" | grep -c .)" -ne 1 ]; then
		adb devices -l
		fail "Cần đúng 1 thiết bị ở trạng thái 'device', hoặc truyền serial: scripts/setup.sh device <serial>"
	fi

	SERIAL="$devices"
}

a() { adb -s "$SERIAL" "$@"; }
sh_() { adb -s "$SERIAL" shell "$@" | tr -d '\r'; }
key_var() { printf 'PCAPDROID_API_KEY_%s' "$(printf '%s' "$SERIAL" | tr -c 'A-Za-z0-9' '_')"; }

# downloads into the cache once and refuses a file whose sha-256 differs from the pin
fetch() {
	local url="$1" name="$2" sha="$3"
	mkdir -p "$CACHE"
	if [ ! -f "$CACHE/$name" ]; then
		curl -fsSL -o "$CACHE/$name.part" "$url/$name"
		mv "$CACHE/$name.part" "$CACHE/$name"
	fi

	local got
	got=$(shasum -a 256 "$CACHE/$name" | awk '{ print $1 }')
	[ "$got" = "$sha" ] || fail "$name: SHA-256 $got khác bản đã kiểm chứng $sha — không cài"
	ok "$name SHA-256 khớp"
}

# reads the whole output first: with pipefail, `| grep -q` on dumpsys's long output kills the
# writer with SIGPIPE and the pipeline fails even when the text is there
vpn_up() { [[ "$(sh_ dumpsys connectivity)" == *"VPN CONNECTED"* ]]; }

installed_version() { sh_ dumpsys package "$1" | awk -F= '/versionName=/ { print $2; exit }'; }

cmd_host() {
	step "Công cụ trên Mac"
	command -v brew >/dev/null || fail "Cần Homebrew: https://brew.sh"
	command -v adb >/dev/null || brew install --cask android-platform-tools
	ok "adb $(adb version | awk 'NR==1 { print $NF }')"
	command -v mitmdump >/dev/null || brew install --cask mitmproxy
	ok "mitmproxy $(mitmdump --version 2>/dev/null | awk '/Mitmproxy/ { print $2 }')"
	command -v node >/dev/null || fail "Cần Node.js >= 20 (brew install node)"
	command -v python3 >/dev/null || fail "Cần python3"
	command -v swiftc >/dev/null && ok "swiftc có sẵn (OCR bật)" || warn "Không có swiftc: OCR tắt (xcode-select --install để bật)"

	step "Build mobile-mcp-lite"
	(cd "$REPO" && npm ci --ignore-scripts --no-fund --silent && npm run build --silent)
	ok "lib/ đã build"
}

cmd_device() {
	pick_serial "${1:-}"
	command -v node >/dev/null && [ -f "$REPO/lib/android.js" ] || fail "Chạy 'scripts/setup.sh host' trước"
	step "Thiết bị $SERIAL: $(sh_ getprop ro.product.manufacturer) $(sh_ getprop ro.product.model), Android $(sh_ getprop ro.build.version.release)"

	step "Appium UiAutomator2 server $UIA2_VERSION (đọc màn hình nhanh, gõ Unicode)"
	if [ "$(installed_version io.appium.uiautomator2.server)" = "$UIA2_VERSION" ] && sh_ pm list packages io.appium.uiautomator2.server.test | grep -c . >/dev/null; then
		ok "đã cài"
	else
		fetch "$UIA2_URL" "$UIA2_SERVER_APK" "$UIA2_SERVER_SHA"
		fetch "$UIA2_URL" "$UIA2_TEST_APK" "$UIA2_TEST_SHA"
		a install -r -t "$CACHE/$UIA2_SERVER_APK" >/dev/null
		a install -r -t "$CACHE/$UIA2_TEST_APK" >/dev/null
		ok "đã cài"
	fi

	step "PCAPdroid (bắt traffic theo app, không cần root)"
	if sh_ pm list packages "$PCAPDROID_PKG" | grep -c "package:$PCAPDROID_PKG\$" >/dev/null; then
		ok "đã cài ($(installed_version $PCAPDROID_PKG))"
	else
		fetch "$PCAPDROID_URL" "$PCAPDROID_APK" "$PCAPDROID_SHA"
		a install "$CACHE/$PCAPDROID_APK" >/dev/null
		ok "đã cài"
	fi

	sh_ pm grant "$PCAPDROID_PKG" android.permission.POST_NOTIFICATIONS 2>/dev/null && ok "cho phép thông báo (hiện icon khi đang capture)" || true
	setup_pcapdroid_key
	approve_vpn_once

	step "Chứng chỉ CA của mitmproxy (chỉ cần nếu muốn giải mã HTTPS)"
	[ -f "$HOME/.mitmproxy/mitmproxy-ca-cert.cer" ] || (mitmdump -q -p 0 >/dev/null 2>&1 & sleep 3; kill $! 2>/dev/null) || true
	if [ -f "$HOME/.mitmproxy/mitmproxy-ca-cert.cer" ]; then
		a push "$HOME/.mitmproxy/mitmproxy-ca-cert.cer" /sdcard/Download/mitmproxy-ca-cert.crt >/dev/null
		ok "đã đẩy file vào Download/mitmproxy-ca-cert.crt"
		manual "Cài đặt > Bảo mật > (Thêm) > Mã hoá & thông tin xác thực > Cài chứng chỉ > Chứng chỉ CA > Vẫn cài > nhập PIN > chọn mitmproxy-ca-cert.crt"
		manual "Kiểm tra: Thông tin xác thực đáng tin cậy > tab Người dùng có 'mitmproxy'. Chỉ làm trên máy test."
		sh_ am start -a android.settings.SECURITY_SETTINGS >/dev/null 2>&1 || true
	else
		warn "Không tạo được CA mitmproxy, bỏ qua"
	fi

	step "Tiếp theo"
	echo "  scripts/setup.sh register   # chỉ cần một lần cho mỗi máy Mac"
	echo "  scripts/setup.sh verify     # kiểm tra lại"
}

# generates a PCAPdroid api key through its settings ui and stores it per device
setup_pcapdroid_key() {
	local var
	var=$(key_var)
	if [ -f "$KEYS" ] && grep -q "^$var=" "$KEYS"; then
		ok "api key đã lưu ($var)"
		return
	fi

	mkdir -p "$CONFIG" && touch "$KEYS" && chmod 600 "$KEYS"
	sh_ am start -n "$PCAPDROID_PKG/.activities.MainActivity" >/dev/null
	sleep 2.5
	# onboarding on first launch, then main screen > Settings > Control permissions > ⋮ > Generate API key
	$UI "$SERIAL" tap-text "SKIP" || true
	$UI "$SERIAL" tap-id "com.android.permissioncontroller:id/permission_allow_button" || true
	local key=""
	if $UI "$SERIAL" tap-text "Settings" && scroll_to "Control permissions" && $UI "$SERIAL" tap-text "Control permissions" \
		&& $UI "$SERIAL" tap-text "More options" && $UI "$SERIAL" tap-text "Generate API key"; then
		key=$($UI "$SERIAL" token || true)
	fi

	if [ -n "$key" ]; then
		printf '%s=%s\n' "$var" "$key" >> "$KEYS"
		$UI "$SERIAL" tap-id "android:id/button1" || true
		ok "đã tạo và lưu api key vào $KEYS ($var)"
	else
		warn "Không tự tạo được api key (giao diện khác ngôn ngữ/phiên bản?)"
		manual "PCAPdroid > Settings > Control permissions > ⋮ > Generate API key, rồi chạy:"
		manual "echo '$var=<key>' >> $KEYS"
	fi

	sh_ input keyevent HOME
}

scroll_to() {
	for _ in 1 2 3 4; do
		$UI "$SERIAL" screen | grep -c "$1" >/dev/null && return 0
		sh_ input swipe 360 1300 360 400 300
		sleep 0.8
	done
	return 1
}

# android asks once per app to allow a vpn; start a short capture and accept the dialogs
approve_vpn_once() {
	local key
	key=$(grep "^$(key_var)=" "$KEYS" 2>/dev/null | cut -d= -f2- || true)
	local auth=()
	[ -n "$key" ] && auth=(-e api_key "$key")
	sh_ am start -e action start ${auth[@]+"${auth[@]}"} -e app_filter com.android.settings -e pcap_dump_mode none -n "$PCAPDROID_CTRL" >/dev/null
	# up to three dialogs, each taking a moment to show: pcapdroid's consent (only without a key, allow_btn),
	# its "simulates a VPN" notice and android's vpn request (both android:id/button1)
	local up=""
	for _ in 1 2 3 4 5 6 7 8 9 10; do
		sleep 1
		vpn_up && up=1 && break
		$UI "$SERIAL" tap-id "$PCAPDROID_PKG:id/allow_btn" || $UI "$SERIAL" tap-id "android:id/button1" || true
	done

	if [ -n "$up" ]; then
		ok "đã cấp quyền VPN cho PCAPdroid"
	else
		warn "VPN chưa bật — mở PCAPdroid và bấm Start một lần để cấp quyền VPN"
	fi

	sh_ am start -e action stop ${auth[@]+"${auth[@]}"} -n "$PCAPDROID_CTRL" >/dev/null
	sleep 1.5
	$UI "$SERIAL" tap-id "$PCAPDROID_PKG:id/allow_btn" >/dev/null 2>&1 || true
	sh_ input keyevent HOME
}

cmd_register() {
	command -v claude >/dev/null || fail "Không thấy lệnh claude"
	claude mcp remove mobile-lite -s user >/dev/null 2>&1 || true
	# api keys stay in $KEYS, which the server reads itself, so ~/.claude.json holds no secret
	claude mcp add -s user mobile-lite -- node "$REPO/lib/index.js" >/dev/null
	ok "đã đăng ký mobile-lite; key PCAPdroid đọc từ $KEYS. Mở phiên Claude Code mới để dùng"
}

cmd_verify() {
	pick_serial "${1:-}"
	step "Kiểm tra $SERIAL"
	[ "$(installed_version io.appium.uiautomator2.server)" = "$UIA2_VERSION" ] && ok "UiAutomator2 $UIA2_VERSION" || warn "UiAutomator2 chưa cài (đọc màn hình chậm hơn, không gõ Unicode)"
	sh_ pm list packages "$PCAPDROID_PKG" | grep -c . >/dev/null && ok "PCAPdroid $(installed_version $PCAPDROID_PKG)" || warn "PCAPdroid chưa cài"
	grep -q "^$(key_var)=" "$KEYS" 2>/dev/null && ok "api key PCAPdroid đã lưu" || warn "chưa có api key PCAPdroid (sẽ phải bấm Allow trên máy)"
	command -v mitmdump >/dev/null && ok "mitmproxy trên Mac" || warn "mitmproxy chưa cài"
	local proxy
	proxy=$(sh_ settings get global http_proxy)
	case "$proxy" in
		null|:0|"") ok "không có proxy treo" ;;
		*) warn "http_proxy=$proxy còn đặt — gỡ: adb -s $SERIAL shell settings put global http_proxy :0" ;;
	esac
	claude mcp get mobile-lite 2>/dev/null | grep -c Connected >/dev/null && ok "MCP mobile-lite đã kết nối" || warn "MCP mobile-lite chưa đăng ký (scripts/setup.sh register)"
	manual "CA mitmproxy: xem Thông tin xác thực đáng tin cậy > Người dùng (không kiểm tra được qua adb khi không root)"
}

cmd_clean() {
	pick_serial "${1:-}"
	step "Gỡ khỏi $SERIAL"
	sh_ settings put global http_proxy :0 >/dev/null; sh_ settings delete global http_proxy >/dev/null || true
	a reverse --remove-all || true
	for p in io.appium.uiautomator2.server io.appium.uiautomator2.server.test "$PCAPDROID_PKG"; do
		a uninstall "$p" >/dev/null 2>&1 && ok "đã gỡ $p" || true
	done
	sh_ rm -f /sdcard/Download/mitmproxy-ca-cert.crt
	local var
	var=$(key_var)
	[ -f "$KEYS" ] && sed -i '' "/^$var=/d" "$KEYS"
	ok "đã xoá key $var"
	manual "Gỡ CA: Thông tin xác thực đáng tin cậy > Người dùng > mitmproxy > Xoá"
}

case "${1:-}" in
	host) cmd_host ;;
	device) cmd_device "${2:-}" ;;
	register) cmd_register ;;
	verify) cmd_verify "${2:-}" ;;
	clean) cmd_clean "${2:-}" ;;
	*) sed -n '2,9p' "$0"; exit 1 ;;
esac
