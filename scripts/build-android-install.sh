#!/usr/bin/env bash
# Build the Spotify DX Android APK and install it to a connected device.
#
# Usage:
#   scripts/build-android-install.sh
#
# Steps: dx bundle -> gradle assembleDebug -> uninstall -> install -> launch.
# Uninstalling first avoids signature/update-incompatible failures.
# NOTE: this wipes the app's local data on the phone.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${TARGET:-aarch64-linux-android}"
PKG="com.example.SpotifyDx"

# Wipe the generated Gradle project (rebuilds from dioxus templates).
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

echo "==> installing $PKG"
adb install "$APK"

echo "==> launching Spotify DX"
adb shell am start -n "$PKG/dev.dioxus.main.MainActivity"

echo "Done. Watch the device."
