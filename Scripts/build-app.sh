#!/bin/zsh
# Builds Xorvio as an optimized, signed app and installs it in /Applications.
#
# It signs with a lasting identity (an "Apple Development" certificate, found
# by signing-identity.sh), so macOS sees every new build as the same app and
# Keychain's "Always Allow" holds from one build to the next. Without one the
# app is signed ad hoc and the Keychain asks again after each build.
#
#   Scripts/build-app.sh                 build, install and open
#   Scripts/build-app.sh --no-open       build and install only
#   Scripts/build-app.sh --with-ffmpeg   with Xorvio's own ffmpeg and ffprobe inside
#                                        (Scripts/build-ffmpeg.sh builds them the first time)
#   Scripts/build-app.sh --release       a release to hand out: ffmpeg inside, signed with a
#                                        Developer ID, notarized, as an app and a disk image
#                                        in target/release/bundle; nothing is installed
#   Scripts/build-app.sh --release --target x86_64-apple-darwin
#                                        the same for Intel Macs, on Apple silicon too, in
#                                        target/x86_64-apple-darwin/release/bundle
#
# A release signs with the Keychain's "Developer ID Application" certificate
# (RELEASE_IDENTITY picks another), and notarizes with notarytool's keychain
# profile NOTARY_PROFILE ("xorvio" unless set), or with App Store Connect API
# key variables (APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH) or Apple
# ID ones (APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID) when they're set. The
# README's "Releases" section says how to make them.
set -euo pipefail

ROOT="${0:A:h:h}"
cd "$ROOT"

OPEN=1
FFMPEG=0
RELEASE=0
TARGET=""
while (( $# )); do
  case "$1" in
    --no-open) OPEN=0 ;;
    --with-ffmpeg) FFMPEG=1 ;;
    --release) RELEASE=1 FFMPEG=1 ;;
    --target) TARGET="${2:-}"; shift ;;
    --target=*) TARGET="${1#*=}" ;;
    *) print -u2 "Unknown option $1 (see the top of Scripts/build-app.sh)"; exit 1 ;;
  esac
  shift
done
if [[ -n "$TARGET" ]] && (( ! RELEASE )); then
  print -u2 "--target is for releases: this Mac runs only what's built for it."
  exit 1
fi

# This Mac's Command Line Tools linker can't link; Xcode's can.
[[ -d /Applications/Xcode.app ]] && export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

if (( RELEASE )); then
  IDENTITY="${RELEASE_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/"Developer ID Application/ { print $2; exit }')}"
  if [[ -z "$IDENTITY" ]]; then
    print -u2 "A release needs a Developer ID Application certificate in the Keychain (README › Releases)."
    exit 1
  fi
  print "Signing with $IDENTITY"
  if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
    NOTARY=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
  elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    NOTARY=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
  else
    NOTARY=(--keychain-profile "${NOTARY_PROFILE:-xorvio}")
  fi
  # Before a long build, so missing credentials show at once.
  if ! xcrun notarytool history "${NOTARY[@]}" >/dev/null; then
    print -u2 "notarytool can't sign in with these credentials (README › Releases)."
    exit 1
  fi
else
  IDENTITY="$("$ROOT/Scripts/signing-identity.sh")"
  if [[ "$IDENTITY" == "-" ]]; then
    print "No signing certificate found; signing ad hoc (the Keychain will ask after each build)."
  else
    print "Signing with $(security find-identity -v -p codesigning | awk -v id="$IDENTITY" '$2 == id { $1 = $2 = ""; print substr($0, 3) }')"
  fi
fi
export APPLE_SIGNING_IDENTITY="$IDENTITY"

TAURI=(--bundles app)
[[ -n "$TARGET" ]] && TAURI+=(--target "$TARGET")
if (( FFMPEG )); then
  zsh "$ROOT/Scripts/build-ffmpeg.sh" ${TARGET:+--target=$TARGET}
  TAURI+=(--config src-tauri/tauri.ffmpeg.conf.json)
fi

if (( RELEASE )); then
  # Tauri notarizes by itself when these are set; here notarytool does it
  # below, for the disk image too.
  env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
    npx tauri build "${TAURI[@]}"
else
  npx tauri build "${TAURI[@]}"
fi

BUNDLE="$ROOT/target${TARGET:+/$TARGET}/release/bundle"
APP="$BUNDLE/macos/Xorvio.app"
codesign --verify --deep --strict "$APP"

if (( RELEASE )); then
  # Notarization takes every executable signed with the hardened runtime
  # and a secure timestamp: the app's and the ffmpeg sidecars'.
  for executable in "$APP"/Contents/MacOS/*; do
    signature="$(codesign -dvv "$executable" 2>&1)"
    if [[ "$signature" != *"(runtime)"* || "$signature" != *"Timestamp="* ]]; then
      print -u2 "${executable:t} isn't signed the way notarization needs (hardened runtime, timestamp)."
      exit 1
    fi
  done

  # notarize <file>: sends it to Apple and waits for the verdict.
  notarize() {
    print "Notarizing ${1:t} (this usually takes a few minutes)"
    local answer="$(xcrun notarytool submit "$1" "${NOTARY[@]}" --wait --output-format json)"
    if [[ ! "$answer" =~ '"status": ?"Accepted"' ]]; then
      print -u2 "Notarization of ${1:t} failed: $answer"
      local id="${${answer#*\"id\":\"}%%\"*}"
      [[ -n "$id" ]] && xcrun notarytool log "$id" "${NOTARY[@]}" >&2
      exit 1
    fi
  }

  OUT="$BUNDLE/release"
  rm -rf "$OUT"
  mkdir -p "$OUT"
  ditto -c -k --keepParent "$APP" "$OUT/Xorvio.zip"
  notarize "$OUT/Xorvio.zip"
  rm "$OUT/Xorvio.zip"
  # The ticket goes on the app too, so it opens offline the first time.
  xcrun stapler staple "$APP"

  VERSION="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1)"
  # Tauri's names: aarch64 for Apple silicon, x64 for Intel.
  ARCH="${${TARGET%%-*}:-$(uname -m)}"
  ARCH="${${ARCH/arm64/aarch64}/x86_64/x64}"
  DMG="$OUT/Xorvio_${VERSION}_${ARCH}.dmg"
  STAGE="$OUT/stage"
  mkdir -p "$STAGE"
  ditto "$APP" "$STAGE/Xorvio.app"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname Xorvio -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
  codesign --sign "$IDENTITY" --timestamp "$DMG"
  notarize "$DMG"
  xcrun stapler staple "$DMG"

  spctl --assess --type execute "$APP"
  spctl --assess --type open --context context:primary-signature "$DMG"
  print "Release ready: $APP and $DMG (Gatekeeper accepts both)."
  exit 0
fi

DEST=/Applications
[[ -w "$DEST" ]] || DEST="$HOME/Applications"
mkdir -p "$DEST"

# Quit any running copy (this one, or a development build) so the new one
# opens rather than the old one coming forward. Recordings in progress stop.
if pgrep -x xorvio >/dev/null; then
  osascript -e 'tell application id "com.xorvio.desktop" to quit' >/dev/null 2>&1 || true
  for _ in {1..20}; do pgrep -x xorvio >/dev/null || break; sleep 0.25; done
fi
rm -rf "$DEST/Xorvio.app"
ditto "$APP" "$DEST/Xorvio.app"
print "Installed $DEST/Xorvio.app"

if (( OPEN )); then
  open "$DEST/Xorvio.app"
fi
