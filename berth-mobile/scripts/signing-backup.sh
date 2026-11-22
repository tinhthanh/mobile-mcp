#!/bin/sh
# Backs up what berth-mobile needs to sign for iPhones, and PCAPdroid keys,
# into one file encrypted with a passphrase you type: AES-256-CBC, key from
# PBKDF2-SHA256 with 600,000 rounds and a random salt (same as vglink's
# secrets.sh). The backup is committed as secrets/berth-mobile-signing.tar.enc;
# this repository is public, so its safety is the passphrase's strength.
#
#   scripts/signing-backup.sh backup [file]   write the encrypted backup
#                                             (default secrets/berth-mobile-signing.tar.enc, replaced)
#   scripts/signing-backup.sh verify <file>   list what the backup holds (asks the passphrase)
#   scripts/signing-backup.sh restore <file>  put it back on a Mac and add the keychain to the search list
#
# Inside: asc/AuthKey_*.p8 + asc.json (App Store Connect API key), signing/
# berth-mobile.keychain-db + keychain.pass + dev.key + dev.cer (Apple
# Development identity), wda.mobileprovision, AppleWWDRCAG3.cer, and
# ~/.config/mobile-mcp-lite/keys.env when present. Losing the passphrase
# means losing what the file holds.
set -eu
STATE="$HOME/Library/Application Support/berth-mobile"
KEYS_DIR="$HOME/.config/mobile-mcp-lite"
CIPHER="-aes-256-cbc -md sha256 -pbkdf2 -iter 600000"

need() { command -v "$1" >/dev/null || { echo "needs $1" >&2; exit 1; }; }
need openssl

case "${1:-}" in
backup)
  out="${2:-$(cd "$(dirname "$0")/.." && pwd)/secrets/berth-mobile-signing.tar.enc}"
  mkdir -p "$(dirname "$out")"
  cd "$STATE"
  files="asc signing/berth-mobile.keychain-db signing/keychain.pass signing/dev.cer signing/wda.mobileprovision signing/AppleWWDRCAG3.cer"
  [ -f signing/dev.key ] && files="$files signing/dev.key"
  for f in $files; do [ -e "$f" ] || { echo "missing $STATE/$f" >&2; exit 1; }; done
  extra=""
  [ -f "$KEYS_DIR/keys.env" ] && extra="-C $KEYS_DIR keys.env"
  umask 077
  # shellcheck disable=SC2086
  COPYFILE_DISABLE=1 tar -czf - $files $extra | openssl enc $CIPHER -salt -out "$out.tmp"
  mv "$out.tmp" "$out"
  echo "wrote $out ($(wc -c < "$out" | tr -d ' ') bytes). Check it with: $0 verify \"$out\""
  ;;
verify)
  [ -f "${2:-}" ] || { echo "usage: $0 verify <file>" >&2; exit 1; }
  # shellcheck disable=SC2086
  openssl enc -d $CIPHER -in "$2" | tar -tzvf -
  ;;
restore)
  [ -f "${2:-}" ] || { echo "usage: $0 restore <file>" >&2; exit 1; }
  umask 077
  mkdir -p "$STATE"
  tmp=$(mktemp -d)
  # shellcheck disable=SC2086
  openssl enc -d $CIPHER -in "$2" | tar -xzf - -C "$tmp"
  for d in asc signing; do
    [ -e "$STATE/$d" ] && { echo "$STATE/$d exists; move it away first" >&2; rm -rf "$tmp"; exit 1; }
  done
  mv "$tmp/asc" "$tmp/signing" "$STATE/"
  if [ -f "$tmp/keys.env" ] && [ ! -e "$KEYS_DIR/keys.env" ]; then
    mkdir -p "$KEYS_DIR" && mv "$tmp/keys.env" "$KEYS_DIR/keys.env"
  fi
  rm -rf "$tmp"
  kc="$STATE/signing/berth-mobile.keychain-db"
  # codesign only looks in the search list: append the keychain, keep the others
  if ! security list-keychains -d user | grep -qF "$kc"; then
    eval "security list-keychains -d user -s $(security list-keychains -d user | tr '\n' ' ') \"$kc\""
  fi
  security unlock-keychain -p "$(cat "$STATE/signing/keychain.pass")" "$kc"
  security find-identity -v -p codesigning "$kc"
  echo "restored into $STATE"
  ;;
*)
  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
  ;;
esac
