#!/bin/zsh
# Prints the code-signing identity the development scripts sign with.
#
# A lasting identity, such as an "Apple Development" certificate, keeps macOS
# recognising each new build as the same app, so Keychain's "Always Allow"
# holds from one build to the next. Without one, builds are signed ad hoc ("-").
# An ad-hoc signature changes with every build, so the Keychain asks for your
# password again after each one.
#
# SIGN_IDENTITY overrides the choice (a certificate's name or SHA-1, or "-").
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  print -r -- "$SIGN_IDENTITY"
  exit 0
fi
identity="$(security find-identity -v -p codesigning 2>/dev/null \
  | awk '/"Apple Development|"Xorvio Local Signing/ { print $2; exit }')"
print -r -- "${identity:--}"
