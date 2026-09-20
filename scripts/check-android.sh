#!/usr/bin/env bash
# Local Android cross-build verification for spotify-dx.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${ARCH:-aarch64-linux-android}"
LINK="${1:---check}"
PKG="com.spotifydx.app"

# NDK toolchain (shared helper — was pasted here and in build-kotlin.sh).
# shellcheck disable=SC1091
source "$(dirname "$0")/android-ndk.sh" "$TARGET"

echo "Verifying $TARGET (headless core + mobile renderer) ..."
# Both feature sets: the shipped Kotlin .so builds headless
# (--no-default-features) while the legacy renderer path needs mobile.
# Checking only one left the other silently broken before.
if [ "$LINK" = "--build" ]; then
  cargo build --no-default-features --target "$TARGET"
  cargo build --no-default-features --features mobile --target "$TARGET"
else
  cargo check --no-default-features --target "$TARGET"
  cargo check --no-default-features --features mobile --target "$TARGET"
fi
echo "Android ($TARGET) cross-build OK."
