#!/usr/bin/env bash
# Build the Spotify DX Android APK and install it to a connected device.
#
# Usage:
#   scripts/build-android-install.sh [debug|release]   (default: debug)
#   DX_LEGACY=1 scripts/build-android-install.sh       (old dx renderer path)
#
# Primary path (docs/KOTLIN_MIGRATION.md): the owned Kotlin project in
# android/ — scripts/build-kotlin.sh (cargo core + gradle assemble), then
# uninstall -> install -> launch. Uninstalling first avoids
# signature/update-incompatible failures.
# NOTE: this wipes the app's local data on the phone.
set -euo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-debug}"
PKG="com.spotifydx.app"

if [ "${DX_LEGACY:-0}" = "1" ]; then
  # Legacy dioxus-mobile renderer path (kept until the Phase 7 cutover).
  TARGET="${TARGET:-aarch64-linux-android}"
  LEGACY_PKG="com.example.SpotifyDx"

  rm -rf target/dx

  echo "==> dx bundle (${TARGET}, release)"
  dx bundle --platform android --release --target "$TARGET" --package-types apk

  echo "==> gradle assembleDebug"
  ( cd target/dx/spotify-dx/release/android/app && ./gradlew :app:assembleDebug --no-daemon )

  APK="$(find target/dx/spotify-dx/release/android/app/app/build/outputs/apk -name '*.apk' | head -1)"
  test -n "$APK" || { echo "no APK produced" >&2; exit 1; }
  echo "==> APK: $APK"

  echo "==> uninstalling old packages (wipes app data)"
  for old_pkg in "com.spotifydx.app" "com.example.SpotifyDx"; do
    adb uninstall "$old_pkg" >/dev/null 2>/dev/null || true
  done

  echo "==> installing $LEGACY_PKG"
  adb install "$APK"

  echo "==> launching Spotify DX (legacy)"
  adb shell am start -n "$LEGACY_PKG/dev.dioxus.main.MainActivity"

  echo "Done. Watch the device."
  exit 0
fi

echo "==> building Kotlin app ($MODE)"
./scripts/build-kotlin.sh "$MODE"

APK="$(find android/app/build/outputs/apk -name '*.apk' | head -1)"
test -n "$APK" || { echo "no APK produced" >&2; exit 1; }
echo "==> APK: $APK"

echo "==> uninstalling old packages (wipes app data)"
for old_pkg in "com.spotifydx.app" "com.example.SpotifyDx"; do
  adb uninstall "$old_pkg" >/dev/null 2>/dev/null || true
done

echo "==> installing $PKG"
adb install "$APK"

echo "==> launching Spotify DX"
adb shell am start -n "$PKG/com.spotifydx.app.MainActivity"

echo "Done. Watch the device."
