#!/usr/bin/env bash

set -euo pipefail

APP_PATH="${1:-target/release/bundle/osx/Inkline.app}"
DMG_PATH="${2:-target/release/bundle/dmg/Inkline.dmg}"
BINARY_PATH="$APP_PATH/Contents/MacOS/inkline"

if [[ ! -d "$APP_PATH" ]]; then
  echo "macOS app bundle not found: $APP_PATH" >&2
  exit 1
fi

if [[ ! -f "$BINARY_PATH" ]]; then
  echo "macOS executable not found: $BINARY_PATH" >&2
  exit 1
fi

ARCHITECTURES="$(lipo -archs "$BINARY_PATH")"
for REQUIRED_ARCHITECTURE in arm64 x86_64; do
  if [[ " $ARCHITECTURES " != *" $REQUIRED_ARCHITECTURE "* ]]; then
    echo "missing $REQUIRED_ARCHITECTURE from macOS executable ($ARCHITECTURES)" >&2
    exit 1
  fi
done

WORK_DIR="$(mktemp -d "${RUNNER_TEMP:-/tmp}/inkline-package.XXXXXX")"
KEYCHAIN_PATH="$WORK_DIR/signing.keychain-db"
CERTIFICATE_PATH="$WORK_DIR/developer-id.p12"
DMG_ROOT="$WORK_DIR/dmg-root"
USING_DEVELOPER_ID=false

cleanup() {
  if [[ -f "$KEYCHAIN_PATH" ]]; then
    security delete-keychain "$KEYCHAIN_PATH" >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

if [[ -n "${MACOS_CERTIFICATE_BASE64:-}" ]]; then
  if [[ -z "${MACOS_CERTIFICATE_PASSWORD:-}" || -z "${MACOS_SIGNING_IDENTITY:-}" ]]; then
    echo "MACOS_CERTIFICATE_PASSWORD and MACOS_SIGNING_IDENTITY are required with MACOS_CERTIFICATE_BASE64" >&2
    exit 1
  fi

  KEYCHAIN_PASSWORD="$(openssl rand -hex 32)"
  printf '%s' "$MACOS_CERTIFICATE_BASE64" | base64 -D > "$CERTIFICATE_PATH"
  security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
  security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"
  security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
  security import "$CERTIFICATE_PATH" \
    -P "$MACOS_CERTIFICATE_PASSWORD" \
    -A \
    -t cert \
    -f pkcs12 \
    -k "$KEYCHAIN_PATH"
  security set-key-partition-list \
    -S apple-tool:,apple:,codesign: \
    -s \
    -k "$KEYCHAIN_PASSWORD" \
    "$KEYCHAIN_PATH"

  codesign --force \
    --options runtime \
    --timestamp \
    --keychain "$KEYCHAIN_PATH" \
    --sign "$MACOS_SIGNING_IDENTITY" \
    "$APP_PATH"
  USING_DEVELOPER_ID=true
else
  echo "Developer ID certificate is not configured; applying a complete ad hoc signature."
  codesign --force --sign - "$APP_PATH"
fi

codesign --verify --deep --strict --verbose=2 "$APP_PATH"

mkdir -p "$DMG_ROOT"
cp -R "$APP_PATH" "$DMG_ROOT/Inkline.app"
ln -s /Applications "$DMG_ROOT/Applications"
mkdir -p "$(dirname "$DMG_PATH")"
hdiutil create \
  -volname Inkline \
  -srcfolder "$DMG_ROOT" \
  -format UDZO \
  -ov \
  "$DMG_PATH"

if [[ "$USING_DEVELOPER_ID" == true ]]; then
  codesign --force \
    --timestamp \
    --keychain "$KEYCHAIN_PATH" \
    --sign "$MACOS_SIGNING_IDENTITY" \
    "$DMG_PATH"
fi

NOTARIZATION_VALUE_COUNT=0
for NOTARIZATION_VALUE in \
  "${APPLE_ID:-}" \
  "${APPLE_APP_SPECIFIC_PASSWORD:-}" \
  "${APPLE_TEAM_ID:-}"; do
  if [[ -n "$NOTARIZATION_VALUE" ]]; then
    NOTARIZATION_VALUE_COUNT=$((NOTARIZATION_VALUE_COUNT + 1))
  fi
done

if [[ "$NOTARIZATION_VALUE_COUNT" -ne 0 && "$NOTARIZATION_VALUE_COUNT" -ne 3 ]]; then
  echo "APPLE_ID, APPLE_APP_SPECIFIC_PASSWORD, and APPLE_TEAM_ID must be configured together" >&2
  exit 1
fi

if [[ "$NOTARIZATION_VALUE_COUNT" -eq 3 ]]; then
  if [[ "$USING_DEVELOPER_ID" != true ]]; then
    echo "notarization requires a Developer ID certificate" >&2
    exit 1
  fi

  xcrun notarytool submit "$DMG_PATH" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait
  xcrun stapler staple "$DMG_PATH"
  xcrun stapler validate "$DMG_PATH"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG_PATH"
fi
