#!/usr/bin/env bash
# Builds Shipyard's "mobile" plugin: dist/mobile/ and dist/mobile-<version>.tar.gz (+ .sha256).
#
#   scripts/package.sh                build a universal macOS binary, pin it, pack the plugin
#   scripts/package.sh --install      also copy dist/mobile to ~/.berth/plugins/mobile
#                                     (Shipyard then asks to allow the new files)
#
# The manifest pins the binary and the skill by SHA-256; everything else the
# plugin uses (adb, UiAutomator2, PCAPdroid) is fetched at run time against
# hashes compiled into the binary.
#
# iPhones: when this Mac has signing material (berth-mobile's signing/, or
# $BERTH_MOBILE_HOME/signing in CI), WebDriverAgent is signed here and shipped
# as wda-signed.zip, its SHA-256 compiled into the binary, so other Macs need
# no keys. WDA_SIGNED=<zip> ships a zip signed elsewhere.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"

VERSION=$(awk -F'"' '/^version =/ { print $2; exit }' Cargo.toml)
OUT="$ROOT/dist/mobile"
rm -rf "$OUT" && mkdir -p "$OUT/bin" "$OUT/skills/berth-mobile"

sha() { shasum -a 256 "$1" | awk '{ print $1 }'; }
SIGN_HOME="${BERTH_MOBILE_HOME:-$HOME/Library/Application Support/berth-mobile}"
unset BERTH_MOBILE_WDA_SHA256
WDA_ZIP="${WDA_SIGNED:-}"
if [ -z "$WDA_ZIP" ] && [ -f "$SIGN_HOME/signing/wda.mobileprovision" ]; then
	cargo build --release --locked
	WDA_ZIP="$ROOT/dist/wda-signed.zip"
	target/release/berth-mobile sign-wda "$WDA_ZIP"
fi
if [ -n "$WDA_ZIP" ]; then
	export BERTH_MOBILE_WDA_SHA256="$(sha "$WDA_ZIP")"
	echo "shipping signed WebDriverAgent $BERTH_MOBILE_WDA_SHA256"
else
	echo "warning: no iPhone signing here: this build ships no signed WebDriverAgent" >&2
fi

for t in aarch64-apple-darwin x86_64-apple-darwin; do
	cargo build --release --locked --target "$t"
done
lipo -create -output "$OUT/bin/berth-mobile" \
	target/aarch64-apple-darwin/release/berth-mobile target/x86_64-apple-darwin/release/berth-mobile
# rust-objcopy can't strip a cross-built slice on every toolchain: strip the fat file here,
# then ad-hoc sign it again (arm64 macOS refuses to run unsigned code)
xcrun strip -x "$OUT/bin/berth-mobile"
codesign --force --sign - "$OUT/bin/berth-mobile"

cp plugin/skills/berth-mobile/SKILL.md "$OUT/skills/berth-mobile/SKILL.md"
# Its icon in Settings → Plugins (Vetgo; Shipyard ignores it).
cp plugin/icon.svg "$OUT/icon.svg"
[ -n "$WDA_ZIP" ] && cp "$WDA_ZIP" "$OUT/wda-signed.zip"
cp ../LICENSE "$OUT/LICENSE"

BIN_SHA=$(sha "$OUT/bin/berth-mobile")
SKILL_SHA=$(sha "$OUT/skills/berth-mobile/SKILL.md")

cat > "$OUT/berth-plugin.json" <<EOF
{
  "id": "mobile",
  "name": "Mobile",
  "version": "$VERSION",
  "description": "Lets agents use an Android phone or an iPhone plugged in over USB: read the screen as compact text, tap, type (Vietnamese ok), swipe, and on Android capture an app's network traffic (berth-mobile $VERSION, darwin-universal).",
  "icon": "icon.svg",
  "author": "tinhthanh",
  "homepage": "https://github.com/tinhthanh/mobile-mcp",
  "mcp": {
    "mobile": {
      "command": "bin/berth-mobile",
      "args": ["mcp"],
      "sha256": "$BIN_SHA"
    }
  },
  "skills": {
    "berth-mobile": {
      "path": "skills/berth-mobile/SKILL.md",
      "sha256": "$SKILL_SHA"
    }
  },
  "setup": {
    "server": "mobile",
    "args": ["setup"],
    "status": ["status"],
    "ready": "ready",
    "problem": "problem",
    "label": "Tải adb (platform-tools 37.0.1) và cài UiAutomator2 lên điện thoại đang cắm"
  },
  "settings": {
    "server": "mobile",
    "list": ["settings"],
    "label": "Điện thoại Android"
  }
}
EOF

TARBALL="$ROOT/dist/mobile-$VERSION.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$TARBALL" -C "$ROOT/dist" mobile
( cd "$ROOT/dist" && shasum -a 256 "mobile-$VERSION.tar.gz" | awk '{ print $1 }' > "mobile-$VERSION.tar.gz.sha256" )

echo "plugin:  $OUT"
echo "tarball: $TARBALL ($(du -h "$TARBALL" | cut -f1)), sha256 $(cat "$TARBALL.sha256")"
"$OUT/bin/berth-mobile" version

if [ "${1:-}" = --install ]; then
	DEST="$HOME/.berth/plugins/mobile"
	rm -rf "$DEST.new" && cp -R "$OUT" "$DEST.new"
	rm -rf "$DEST" && mv "$DEST.new" "$DEST"
	echo "installed: $DEST (allow it in Shipyard: Settings → Plugins)"
fi
