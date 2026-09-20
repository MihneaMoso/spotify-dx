#!/usr/bin/env bash
# Build the Kotlin Android app (the primary Android build path —
# docs/KOTLIN_MIGRATION.md §13). No Android Studio, no dx generator: the owned
# Gradle project in android/ plus the native core cross-compiled with the NDK
# toolchain already on this machine.
#
# Usage:
#   scripts/build-kotlin.sh [debug|release]   (default: debug)
#
# Steps: cargo build (aarch64-linux-android, headless core) -> copy
# libspotify_dx.so into app jniLibs -> gradle assemble. Uses only the
# pre-installed NDK/JDK/SDK; Gradle resolves plugins + deps from the local
# cache (`--offline`) so the build installs nothing.
set -euo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-debug}"
TARGET="aarch64-linux-android"
ABI="arm64-v8a"

# --- NDK toolchain (shared helper — was pasted here and in check-android.sh) ---
# shellcheck disable=SC1091
source "$(dirname "$0")/android-ndk.sh" "$TARGET"
# Resource caps: bound codegen units + link jobs so rebuilds don't hog the
# machine (Gradle side is capped in android/gradle.properties). Incremental
# compilation stays on (default for debug); the shared target/ dir is the
# cache — never clean it for an incremental build.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
export CARGO_INCREMENTAL=1

# --- Native core (headless lib; the Kotlin app links it, not the binary) ---
PROFILE_FLAG=""
GRADLE_TASK="assembleDebug"
if [ "$MODE" = "release" ]; then
  PROFILE_FLAG="--release"
  GRADLE_TASK="assembleRelease"
fi

echo "==> cargo build (native core, $TARGET, $MODE)"
# shellcheck disable=SC2086
cargo build --no-default-features --target "$TARGET" $PROFILE_FLAG

SO="target/$TARGET/$([ "$MODE" = "release" ] && echo release || echo debug)/libspotify_dx.so"
test -f "$SO" || { echo "missing $SO" >&2; exit 1; }

JNILIBS="android/app/build/nativeLibs/$ABI"
mkdir -p "$JNILIBS"
cp -f "$SO" "$JNILIBS/libspotify_dx.so"
echo "==> staged $SO -> $JNILIBS/"
# Strip debug symbols from the STAGED copy only: target/ keeps full symbols
# for native debugging, and symbols never execute (zero runtime effect).
# Saves ~330MB per APK. NDK llvm-strip sits next to the clang above.
"$NDK_BIN/llvm-strip" --strip-debug "$JNILIBS/libspotify_dx.so"
echo "==> stripped staged lib ($(stat -c%s "$JNILIBS/libspotify_dx.so") bytes)"

# --- Bridge compat: every Kotlin-declared native symbol must exist in the .so ---
echo "==> bridge symbol check"
syms="$(grep -rhoE "Java_com_spotifydx_app_CoreBridge_[A-Za-z0-9_]+" android/app/src/main/java/ | sort -u)"
test -n "$syms" || { echo "bridge compat FAILED: no Kotlin-declared symbols found (moved sources?)" >&2; exit 1; }
missing=0
for sym in $syms; do
  if ! nm -D --defined-only "$SO" | grep -q " $sym\$"; then
    echo "MISSING symbol in libspotify_dx.so: $sym" >&2
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  echo "bridge compat FAILED" >&2
  exit 1
fi
echo "bridge compat OK"

# --- Gradle (owned project; offline = cache only, installs nothing) ---
echo "==> gradle $GRADLE_TASK"
(
  cd android
  # Offline by default (local loop installs nothing); CI exports
  # GRADLE_OFFLINE=0 for first-time dependency resolution.
  OFFLINE_FLAG="--offline"
  [ "${GRADLE_OFFLINE:-1}" = "0" ] && OFFLINE_FLAG=""
  # Drop the previous APK first: AGP updates the zip in place and leaves
  # hundreds of MB of orphaned bytes behind across incremental builds
  # (dead space inside the file, invisible to unzip -l). Deleting only the
  # outputs re-runs packaging (~seconds); compilation stays incremental.
  APK_OUT="app/build/outputs/apk/$([ "$MODE" = "release" ] && echo release || echo debug)"
  rm -rf "$APK_OUT"
  # shellcheck disable=SC2086
  ./gradlew "$GRADLE_TASK" $OFFLINE_FLAG --no-daemon
)

APK_DIR="android/app/build/outputs/apk/$([ "$MODE" = "release" ] && echo release || echo debug)"
APK="$(find "$APK_DIR" -name '*.apk' | head -1)"
test -n "${APK:-}" || { echo "no APK produced" >&2; exit 1; }
echo "==> APK: $APK"
# Single-entry guard: incremental packaging once kept a stale duplicate .so
# (+348MB). Fail loudly instead of shipping bloat.
SO_COUNT="$(unzip -l "$APK" | grep -c "lib/$ABI/libspotify_dx.so")"
[ "$SO_COUNT" = "1" ] || { echo "duplicate native lib entries: $SO_COUNT" >&2; exit 1; }
echo "==> native lib entries: $SO_COUNT"
