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

# --- NDK toolchain (same trick as scripts/check-android.sh) ---
NDK_BIN=""
for cand in "${ANDROID_NDK_HOME:-}" "${NDK_HOME:-}" "${ANDROID_HOME:-}/ndk/"*/ "${ANDROID_SDK_ROOT:-}/ndk/"*/; do
  [ -z "$cand" ] && continue
  b="$cand/toolchains/llvm/prebuilt/linux-x86_64/bin"
  [ -x "$b/clang" ] && NDK_BIN="$b" && break
done
if [ -z "$NDK_BIN" ]; then
  echo "Android NDK toolchain not found (set ANDROID_NDK_HOME)." >&2
  exit 1
fi

case "$TARGET" in
  # Match Dioxus.toml [mobile] min_sdk_version = 30 so link-time system stubs
  # (notably libaaudio.so) resolve from the selected Android API sysroot.
  aarch64-linux-android) TRIPLE="aarch64-linux-android30" ;;
  *) echo "Unsupported target: $TARGET" >&2; exit 1 ;;
esac

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
cat > "$workdir/$TARGET-clang" <<EOF
#!/bin/bash
exec "$NDK_BIN/clang" --target=$TRIPLE "\$@"
EOF
chmod +x "$workdir/$TARGET-clang"
# C build scripts (cc-rs, e.g. ring) probe `aarch64-linux-android-ar` on
# PATH — they do NOT read CARGO_TARGET_*_AR — and stock NDK bins only carry
# llvm-ar. Same wrapper trick as clang (this exact gap broke CI release).
cat > "$workdir/$TARGET-ar" <<EOF
#!/bin/bash
exec "$NDK_BIN/llvm-ar" "\$@"
EOF
chmod +x "$workdir/$TARGET-ar"

export PATH="$workdir:$PATH"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$workdir/$TARGET-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_CC="$workdir/$TARGET-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR="$workdir/$TARGET-ar"
export AR_aarch64_linux_android="$workdir/$TARGET-ar"
export CC_aarch64_linux_android="$workdir/$TARGET-clang"
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

JNILIBS="android/app/src/main/jniLibs/$ABI"
mkdir -p "$JNILIBS"
cp -f "$SO" "$JNILIBS/libspotify_dx.so"
echo "==> staged $SO -> $JNILIBS/"

# --- Bridge compat: every Kotlin-declared native symbol must exist in the .so ---
echo "==> bridge symbol check"
missing=0
for sym in $(grep -rhoE "Java_com_spotifydx_app_CoreBridge_[A-Za-z0-9_]+" android/app/src/main/java/ | sort -u); do
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
  # shellcheck disable=SC2086
  ./gradlew "$GRADLE_TASK" $OFFLINE_FLAG --no-daemon
)

APK_DIR="android/app/build/outputs/apk/$([ "$MODE" = "release" ] && echo release || echo debug)"
APK="$(find "$APK_DIR" -name '*.apk' | head -1)"
test -n "${APK:-}" || { echo "no APK produced" >&2; exit 1; }
echo "==> APK: $APK"
