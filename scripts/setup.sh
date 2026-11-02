#!/usr/bin/env bash
# Set up mobile-mcp-lite on a Mac and its Android phones. See docs/SETUP.md.
#
#   scripts/setup.sh                 everything, safe to re-run: Mac tools + build, every
#                                    connected phone (waits for one), Claude Code registration
#     --net                          also network capture: mitmproxy, PCAPdroid + api key + VPN consent
#     --with-ca                      also push the mitmproxy CA for HTTPS decryption (installed by hand)
#     --serial <id>                  only this phone
#     --yes                          don't ask (adds the mcp__mobile-lite permission rule)
#     --no-register                  skip the Claude Code registration
#
#   scripts/setup.sh host            Mac tools and build only
#   scripts/setup.sh device [serial] one phone only (accepts --net, --with-ca)
#   scripts/setup.sh register        Claude Code registration only
#   scripts/setup.sh verify [serial] check a phone
#   scripts/setup.sh clean [serial]  remove what setup installed on a phone (the user CA by hand)
#
# Every APK is pinned to the SHA-256 that was checked against its official release
# (UiAutomator2: GitHub release == npm package apks, the npm package has SLSA provenance).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
UI="node $REPO/scripts/device-ui.cjs"
CACHE="$HOME/Library/Caches/mobile-mcp-lite/apk"
CONFIG="$HOME/.config/mobile-mcp-lite"
KEYS="$CONFIG/keys.env"
CLAUDE_SETTINGS="$HOME/.claude/settings.json"
BUILD_STAMP="$REPO/lib/.setup-stamp"
DEVICE_WAIT_SECONDS="${DEVICE_WAIT_SECONDS:-300}"

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

NET=""
WITH_CA=""
YES=""
REGISTER=1
SERIAL=""
SUMMARY=()

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }
manual() { printf '  \033[36m→ LÀM TAY:\033[0m %s\n' "$*"; }

# yes/no question; --yes answers yes, no terminal answers no
ask() {
	[ -n "$YES" ] && return 0
	[ -t 0 ] || return 1
	local answer
	read -r -p "  ? $1 [y/N] " answer
	[[ "$answer" == [yY]* ]]
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

has_package() { [[ "$(sh_ pm list packages "$1")" == *"package:$1"* ]]; }

# ---- Mac ---------------------------------------------------------------------------------

cmd_host() {
	step "Công cụ trên Mac"
	command -v brew >/dev/null || fail "Cần Homebrew: https://brew.sh"
	command -v adb >/dev/null || brew install --cask android-platform-tools
	ok "adb $(adb version | awk 'NR==1 { print $NF }')"
	command -v node >/dev/null || brew install node
	ok "node $(node --version)"
	command -v python3 >/dev/null || fail "Cần python3 (xcode-select --install)"
	if command -v swiftc >/dev/null; then ok "swiftc có sẵn (OCR bật)"; else warn "Không có swiftc: OCR tắt (xcode-select --install để bật)"; fi
	if [ -n "$NET" ]; then
		command -v mitmdump >/dev/null || brew install --cask mitmproxy
		ok "mitmproxy $(mitmdump --version 2>/dev/null | awk '/Mitmproxy/ { print $2 }')"
	fi

	step "Build mobile-mcp-lite"
	# skip npm ci and the build when nothing they depend on changed
	local stamp
	stamp=$(cd "$REPO" && cat package-lock.json tsconfig.json $(find src native -type f | sort) | shasum -a 256 | cut -c1-16)
	if [ -f "$REPO/lib/index.js" ] && [ "$(cat "$BUILD_STAMP" 2>/dev/null)" = "$stamp" ]; then
		ok "lib/ đã mới nhất"
	else
		(cd "$REPO" && npm ci --ignore-scripts --no-fund --silent && npm run build --silent)
		printf '%s' "$stamp" > "$BUILD_STAMP"
		ok "lib/ đã build"
	fi
}

# ---- phones ------------------------------------------------------------------------------

# prints the serials of authorized phones, waiting (with instructions) until there is one
wait_for_devices() {
	local deadline=$(( $(date +%s) + DEVICE_WAIT_SECONDS )) said=""
	while :; do
		local list ready waiting
		list=$(adb devices | awk 'NR > 1 && NF >= 2 { print $1, $2 }')
		ready=$(printf '%s\n' "$list" | awk '$2 == "device" { print $1 }')
		[ -n "$SERIAL" ] && ready=$(printf '%s\n' "$ready" | grep -x "$SERIAL" || true)
		if [ -n "$ready" ]; then
			printf '%s\n' "$ready"
			return 0
		fi

		waiting=$(printf '%s\n' "$list" | awk '{ print $2 }' | sort -u | tr '\n' ' ')
		local message
		case "$waiting" in
			*unauthorized*) message="Mở khoá điện thoại và bấm Allow (Cho phép) ở hộp thoại 'Allow USB debugging?' — nên tick 'Always allow'" ;;
			*offline*) message="Điện thoại đang offline: rút cáp cắm lại, hoặc tắt/bật Gỡ lỗi USB" ;;
			*) message="Cắm điện thoại bằng cáp dữ liệu; bật Tuỳ chọn nhà phát triển (bấm 7 lần vào Số hiệu bản dựng) và Gỡ lỗi USB" ;;
		esac
		if [ "$message" != "$said" ]; then
			manual "$message" >&2
			said="$message"
		fi
		[ "$(date +%s)" -lt "$deadline" ] || fail "Không thấy điện thoại sau ${DEVICE_WAIT_SECONDS}s (DEVICE_WAIT_SECONDS để chờ lâu hơn)"
		sleep 2
	done
}

pick_serial() {
	[ -n "${1:-}" ] && SERIAL="$1"
	local ready
	ready=$(wait_for_devices)
	if [ "$(printf '%s\n' "$ready" | grep -c .)" -ne 1 ]; then
		adb devices -l
		fail "Có nhiều điện thoại: truyền serial, ví dụ scripts/setup.sh $CMD <serial>"
	fi
	SERIAL="$ready"
}

cmd_device() {
	pick_serial "${1:-}"
	device_setup
}

# installs and checks what one phone needs; SERIAL is set
device_setup() {
	[ -f "$REPO/lib/android.js" ] || fail "Chạy 'scripts/setup.sh host' trước"
	local maker model
	maker=$(sh_ getprop ro.product.manufacturer)
	model=$(sh_ getprop ro.product.model)
	[[ "$model" == "$maker"* ]] || model="$maker $model"
	model="$model, Android $(sh_ getprop ro.build.version.release)"
	step "Điện thoại $SERIAL: $model"

	if [ "$(installed_version io.appium.uiautomator2.server)" = "$UIA2_VERSION" ] && has_package io.appium.uiautomator2.server.test; then
		ok "UiAutomator2 $UIA2_VERSION đã cài"
	else
		fetch "$UIA2_URL" "$UIA2_SERVER_APK" "$UIA2_SERVER_SHA"
		fetch "$UIA2_URL" "$UIA2_TEST_APK" "$UIA2_TEST_SHA"
		a install -r -t "$CACHE/$UIA2_SERVER_APK" >/dev/null
		a install -r -t "$CACHE/$UIA2_TEST_APK" >/dev/null
		ok "UiAutomator2 $UIA2_VERSION đã cài"
	fi

	# a real read, not just "installed": starts the server and fetches the screen
	local read_ms uia2="lỗi"
	if read_ms=$($UI "$SERIAL" uia2-check 2>/dev/null); then
		uia2="chạy được (khởi động + đọc màn hình ${read_ms}ms)"
		ok "UiAutomator2 $uia2"
	else
		warn "UiAutomator2 không chạy được — mobile-lite sẽ dùng uiautomator dump (chậm hơn, không gõ Unicode)"
	fi

	local net="—"
	if [ -n "$NET" ]; then
		net=$(net_setup)
	fi

	if [ -n "$WITH_CA" ]; then
		push_ca
	fi

	SUMMARY+=("$SERIAL|$model|$uia2|$net")
}

# prints a one-word status for the summary; progress goes to stderr
net_setup() {
	{
		if has_package "$PCAPDROID_PKG"; then
			ok "PCAPdroid đã cài ($(installed_version $PCAPDROID_PKG))"
		else
			fetch "$PCAPDROID_URL" "$PCAPDROID_APK" "$PCAPDROID_SHA"
			a install "$CACHE/$PCAPDROID_APK" >/dev/null
			ok "PCAPdroid đã cài"
		fi
		sh_ pm grant "$PCAPDROID_PKG" android.permission.POST_NOTIFICATIONS 2>/dev/null && ok "PCAPdroid được hiện thông báo khi đang capture" || true
		setup_pcapdroid_key
	} >&2
	if approve_vpn_once >&2; then
		echo "sẵn sàng"
	else
		echo "cần bấm Start trong PCAPdroid một lần"
	fi
}

push_ca() {
	[ -f "$HOME/.mitmproxy/mitmproxy-ca-cert.cer" ] || (mitmdump -q -p 0 >/dev/null 2>&1 & sleep 3; kill $! 2>/dev/null) || true
	if [ -f "$HOME/.mitmproxy/mitmproxy-ca-cert.cer" ]; then
		a push "$HOME/.mitmproxy/mitmproxy-ca-cert.cer" /sdcard/Download/mitmproxy-ca-cert.crt >/dev/null 2>&1
		ok "đã đẩy CA vào Download/mitmproxy-ca-cert.crt"
		manual "Cài đặt > Bảo mật > (Thêm) > Mã hoá & thông tin xác thực > Cài chứng chỉ > Chứng chỉ CA > Vẫn cài > nhập PIN > chọn mitmproxy-ca-cert.crt"
		manual "Kiểm tra: Thông tin xác thực đáng tin cậy > tab Người dùng có 'mitmproxy'. Chỉ làm trên máy test."
		sh_ am start -a android.settings.SECURITY_SETTINGS >/dev/null 2>&1 || true
	else
		warn "Không tạo được CA mitmproxy (cần --net để cài mitmproxy)"
	fi
}

# generates a PCAPdroid api key through its settings ui and stores it per device
setup_pcapdroid_key() {
	local var
	var=$(key_var)
	if [ -f "$KEYS" ] && grep -q "^$var=" "$KEYS"; then
		ok "api key PCAPdroid đã lưu ($var)"
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
		ok "đã tạo api key PCAPdroid, lưu ở $KEYS ($var)"
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

# android asks once per app to allow a vpn; start a short capture and accept the dialogs.
# Also proves the api key works: with a valid key no consent prompt shows.
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

	sh_ am start -e action stop ${auth[@]+"${auth[@]}"} -n "$PCAPDROID_CTRL" >/dev/null
	sleep 1.5
	$UI "$SERIAL" tap-id "$PCAPDROID_PKG:id/allow_btn" >/dev/null 2>&1 || true
	sh_ input keyevent HOME

	if [ -n "$up" ]; then
		ok "PCAPdroid bắt được traffic (quyền VPN đã cấp)"
	else
		warn "VPN chưa bật — mở PCAPdroid và bấm Start một lần để cấp quyền VPN"
		return 1
	fi
}

# ---- Claude Code -------------------------------------------------------------------------

cmd_register() {
	step "Claude Code"
	if ! command -v claude >/dev/null; then
		warn "Không thấy lệnh claude; cài Claude Code rồi chạy lại: scripts/setup.sh register"
		return
	fi

	# re-register only when missing or pointing at another checkout
	if [[ "$(claude mcp get mobile-lite 2>/dev/null)" == *"$REPO/lib/index.js"* ]]; then
		ok "MCP mobile-lite đã đăng ký"
	else
		claude mcp remove mobile-lite -s user >/dev/null 2>&1 || true
		# api keys stay in $KEYS, which the server reads itself, so ~/.claude.json holds no secret
		claude mcp add -s user mobile-lite -- node "$REPO/lib/index.js" >/dev/null
		ok "đã đăng ký MCP mobile-lite"
	fi

	mkdir -p "$HOME/.claude/agents"
	ln -sfn "$REPO/.claude/agents/android-tester.md" "$HOME/.claude/agents/android-tester.md"
	ok "subagent android-tester (~/.claude/agents)"

	if python3 -I -c 'import json,sys; sys.exit(0 if "mcp__mobile-lite" in json.load(open(sys.argv[1])).get("permissions", {}).get("allow", []) else 1)' "$CLAUDE_SETTINGS" 2>/dev/null; then
		ok "quyền mcp__mobile-lite đã có (agent không bị hỏi từng thao tác)"
	elif ask "Cho phép Claude Code dùng tool mobile-lite không cần hỏi từng lần (thêm mcp__mobile-lite vào $CLAUDE_SETTINGS)?"; then
		mkdir -p "$(dirname "$CLAUDE_SETTINGS")"
		[ -f "$CLAUDE_SETTINGS" ] && cp "$CLAUDE_SETTINGS" "$CLAUDE_SETTINGS.bak"
		python3 -I - "$CLAUDE_SETTINGS" <<'PY'
import json, os, sys
path = sys.argv[1]
data = json.load(open(path)) if os.path.exists(path) else {}
allow = data.setdefault("permissions", {}).setdefault("allow", [])
if "mcp__mobile-lite" not in allow:
    allow.append("mcp__mobile-lite")
with open(path, "w") as f:
    json.dump(data, f, indent=2, ensure_ascii=False)
    f.write("\n")
PY
		ok "đã thêm quyền mcp__mobile-lite (bản cũ: $CLAUDE_SETTINGS.bak)"
	else
		warn "chưa thêm quyền mcp__mobile-lite: Claude Code sẽ hỏi trước mỗi thao tác trên điện thoại"
	fi
}

# ---- everything --------------------------------------------------------------------------

cmd_up() {
	cmd_host

	step "Điện thoại"
	local serials
	serials=$(wait_for_devices)
	for SERIAL in $serials; do
		device_setup
	done

	[ -n "$REGISTER" ] && cmd_register

	step "Xong"
	local row
	for row in "${SUMMARY[@]}"; do
		IFS='|' read -r serial model uia2 net <<<"$row"
		printf '  %s  %s\n      UiAutomator2: %s · bắt network: %s\n' "$serial" "$model" "$uia2" "$net"
	done
	echo "  Mở phiên Claude Code mới và thử: \"Dùng android-tester mở Cài đặt và cho biết Dark mode đang bật hay tắt\""
	[ -n "$NET" ] || echo "  Cần bắt network? chạy lại với --net"
}

cmd_verify() {
	pick_serial "${1:-}"
	step "Kiểm tra $SERIAL"
	if [ "$(installed_version io.appium.uiautomator2.server)" = "$UIA2_VERSION" ]; then
		local read_ms
		if read_ms=$($UI "$SERIAL" uia2-check 2>/dev/null); then ok "UiAutomator2 $UIA2_VERSION đọc màn hình được (${read_ms}ms)"; else warn "UiAutomator2 đã cài nhưng không chạy được"; fi
	else
		warn "UiAutomator2 chưa cài (đọc màn hình chậm hơn, không gõ Unicode)"
	fi
	if has_package "$PCAPDROID_PKG"; then ok "PCAPdroid $(installed_version $PCAPDROID_PKG)"; else warn "PCAPdroid chưa cài (chỉ cần với --net)"; fi
	if grep -q "^$(key_var)=" "$KEYS" 2>/dev/null; then ok "api key PCAPdroid đã lưu"; else warn "chưa có api key PCAPdroid (chỉ cần với --net)"; fi
	local proxy
	proxy=$(sh_ settings get global http_proxy)
	case "$proxy" in
		null|:0|"") ok "không có proxy treo" ;;
		*) warn "http_proxy=$proxy còn đặt — gỡ: adb -s $SERIAL shell settings put global http_proxy :0" ;;
	esac
	if claude mcp get mobile-lite 2>/dev/null | grep -c Connected >/dev/null; then ok "MCP mobile-lite đã kết nối"; else warn "MCP mobile-lite chưa đăng ký (scripts/setup.sh register)"; fi
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
	manual "Gỡ CA (nếu đã cài): Thông tin xác thực đáng tin cậy > Người dùng > mitmproxy > Xoá"
}

CMD="up"
POSITIONAL=()
while [ $# -gt 0 ]; do
	case "$1" in
		--net) NET=1 ;;
		--with-ca) WITH_CA=1 ;;
		--yes|-y) YES=1 ;;
		--no-register) REGISTER="" ;;
		--serial) SERIAL="${2:?--serial cần giá trị}"; shift ;;
		-h|--help) sed -n '2,20p' "$0"; exit 0 ;;
		host|device|register|verify|clean|up) CMD="$1" ;;
		-*) fail "Không biết tuỳ chọn $1 (xem --help)" ;;
		*) POSITIONAL+=("$1") ;;
	esac
	shift
done
[ "${#POSITIONAL[@]}" -gt 0 ] && SERIAL="${POSITIONAL[0]}"

case "$CMD" in
	up) cmd_up ;;
	host) cmd_host ;;
	device) [ -n "$NET" ] && cmd_host; cmd_device "$SERIAL" ;;
	register) cmd_register ;;
	verify) cmd_verify "$SERIAL" ;;
	clean) cmd_clean "$SERIAL" ;;
esac
