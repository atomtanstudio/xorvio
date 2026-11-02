#!/bin/zsh
# Cargo runs binaries through this on macOS (see .cargo/config.toml). The
# app's own binary is signed with the lasting identity and the app's bundle
# id first, so `npm run tauri dev` builds count as the same app as the
# installed one and Keychain's "Always Allow" holds. Everything else (tests,
# tools) runs as it is.
binary="$1"
if [[ "${binary:t}" == "xorvio" ]]; then
  identity="$("${0:A:h}/signing-identity.sh")"
  codesign --force --sign "$identity" --identifier com.xorvio.desktop "$binary" 2>/dev/null || true
fi
exec "$@"
